//! Wallet state sync module
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use bitcoin::hashes::{sha256::Hash as Sha256Hash, FromSliceError};
use btcserverlib::extended_client::{BtcServerExtendedApi, GrpcClientError};
use client::{GetFinalizedPegoutIdsResponse, ResetWalletStateRequest};
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_data_parser::{DataParser, Error as CompressorError, SerializationType};
use reth_evm::execute::BlockExecutorProvider;
use reth_network::frost::{
    manager::{FrostCommand, FrostConfig, ToFrostManager},
    PeerMessageResponse,
};
use reth_network_peers::PeerId;
use reth_primitives::extra_data_header::ExtraDataHeaderDeserializeError;
use reth_provider::{
    BlockReaderIdExt, CanonStateNotification, CanonStateSubscriptions, ProviderError,
};
use reth_tasks::TaskExecutor;
use tokio::sync::{mpsc::error::SendError, RwLock};
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

use crate::{prost_parser::ProstMessageSerdelizer, Storage};

#[derive(Debug, thiserror::Error)]
/// Wallet state synchronization errors
pub enum WalletStateSyncError {
    #[error("db provider error: {0}")]
    /// Latest block error
    LatestBlockError(#[from] ProviderError),
    #[error("deserilaize extra data header : {0}")]
    /// Extra data header deserialize error
    DeserializeExtraDataHeaderError(#[from] ExtraDataHeaderDeserializeError),
    #[error("btc server client error: {0}")]
    /// Btc server client error
    BtcServerClientError(#[from] GrpcClientError),
    #[error("frost manager send error: {0}")]
    /// Frost manager send error
    FrostManagerSendError(#[from] SendError<FrostCommand>),
    #[error("peer never responded with wallet state, timer elapsed")]
    /// Peer wallet state timeout
    PeerWalletStateTimeout,
    #[error("Failed to receive a frost message from a peer {0}")]
    /// Frost recv error
    FrostRecv(tokio::sync::oneshot::error::RecvError),
    #[error("Failed to decompress wallet state data {0}")]
    /// Compressor error
    CompressorError(#[from] CompressorError),
    #[error("UTXO set from peer is not in sync with the latest block, current utxo set merkel root: {0}, latest utxo set merkel root: {1}")]
    /// Utxo set not in sync
    UtxoSetNotInSync(Sha256Hash, Sha256Hash),
    #[error("Failed to convert slide to sha256 hash {0}")]
    /// Sha256 hash error
    Sha256HashError(#[from] FromSliceError),
}

/// Trait for synchronizing wallet state
#[allow(async_fn_in_trait)]
pub trait WalletStateSync {
    /// Synchronizes the wallet state
    async fn sync_wallet_state(&self) -> Result<(), WalletStateSyncError>;
}

/// Struct for wallet state sync peer response
/// This struct is used to store the wallet state received from a peer
#[derive(Debug, Clone, Default)]
pub struct WalletStateSyncPeerResponse {
    /// Data received from the peer
    data: Vec<Vec<u8>>,
    /// Set of chunks received from the peer
    total_chunks_received: HashSet<u64>,
    /// Total number of chunks expected from the peer
    total_chunks_expected: u64,
}

impl WalletStateSyncPeerResponse {
    /// Creates a new WalletStateSyncPeerResponse
    pub fn new() -> Self {
        Self { data: Vec::new(), total_chunks_received: HashSet::new(), total_chunks_expected: 0 }
    }
    /// Checks if all chunks have been received
    pub fn all_chunks_received(&self) -> bool {
        self.total_chunks_received.len() == self.total_chunks_expected as usize
    }

    /// Appends chunk data to the response
    pub fn append_received_data(&mut self, partial_data: &Vec<Vec<u8>>) {
        self.data.extend_from_slice(partial_data);
    }

    /// Sets the total number of chunks expected
    pub fn set_chunks_expected(&mut self, total_chunks_expected: u64) {
        self.total_chunks_expected = total_chunks_expected;
    }

    /// Adds a chunk index to the set of chunks received
    pub fn add_chunk_received(&mut self, chunk_index_received: u64) {
        self.total_chunks_received.insert(chunk_index_received);
    }
}

type WalletStateSyncResponseCycle =
    Arc<RwLock<Option<(Uuid, HashMap<PeerId, WalletStateSyncPeerResponse>)>>>;

/// Returns an iterator over the fully synced peers
pub fn get_fully_synced_peers<'a>(
    peers_wallet_state_sync_responses: &'a HashMap<PeerId, WalletStateSyncPeerResponse>,
) -> (impl Iterator<Item = (&'a PeerId, &'a WalletStateSyncPeerResponse)> + 'a, usize) {
    let fully_synced = peers_wallet_state_sync_responses
        .iter()
        .filter(|(_, response)| response.all_chunks_received())
        .collect::<Vec<_>>();

    let count = fully_synced.len();
    (fully_synced.into_iter(), count)
}

#[derive(Clone)]
/// Engine for synchronizing wallet state
pub struct WalletStateSyncEngine<EF, BF, DB, ToFrostMan, BtcServerClient> {
    storage: Storage<EF, BF, DB>,
    btc_server: BtcServerClient,
    to_frost_manager: ToFrostMan,
    data_parser: DataParser,
    task_executor: TaskExecutor,
    frost_config: FrostConfig,
    current_response_cycle: WalletStateSyncResponseCycle,
}

impl<EF, BF, DB, ToFrostMan, BtcServerClient>
    WalletStateSyncEngine<EF, BF, DB, ToFrostMan, BtcServerClient>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    ToFrostMan: ToFrostManager + Sync + Clone + 'static,
    DB: BlockReaderIdExt + CanonStateSubscriptions + Clone + 'static,
    BtcServerClient: BtcServerExtendedApi + Clone,
{
    pub(crate) fn new(
        storage: Storage<EF, BF, DB>,
        btc_server: BtcServerClient,
        to_frost_manager: ToFrostMan,
        task_executor: TaskExecutor,
        frost_config: FrostConfig,
    ) -> Self {
        let data_parser =
            DataParser::default().with_serialization_type(SerializationType::Postcard);
        Self {
            storage,
            btc_server,
            to_frost_manager,
            data_parser,
            task_executor,
            frost_config,
            current_response_cycle: Default::default(),
        }
    }
}

impl<EF, BF, DB, ToFrostMan, BtcServerClient> WalletStateSync
    for WalletStateSyncEngine<EF, BF, DB, ToFrostMan, BtcServerClient>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    ToFrostMan: ToFrostManager + Clone + Sync + 'static,
    DB: BlockReaderIdExt + CanonStateSubscriptions + Clone + 'static,
    BtcServerClient: BtcServerExtendedApi + Clone,
{
    // Note: this function should not be called unless we are fully synced
    async fn sync_wallet_state(&self) -> Result<(), WalletStateSyncError> {
        trace!(target: "consensus::authority::UTXOSync::sync_utxo_set", "syncing utxo set");
        let mut btc_server = self.btc_server.clone();

        let (peer_messages_tx, peer_messages_rx) = tokio::sync::oneshot::channel();

        self.to_frost_manager
            .send_command(FrostCommand::GetPeerMessagesStream(peer_messages_tx))?;
        let mut peer_messages_rx = peer_messages_rx.await.expect("peer messages rx to be open");

        let data_parser = self.data_parser.clone();
        let frost_config = self.frost_config.clone();
        let current_response_cycle = self.current_response_cycle.clone();
        let mut canon_events = self.storage.client.subscribe_to_canonical_state();

        self.task_executor.clone().spawn(async move {
            // try getting the wallet state from the peers we requested it from
            loop {
                match peer_messages_rx.recv().await {
                    Some(peer_message_context) => {
                        info!(target: "consensus::authority::sync_wallet_state", "Received wallet state from peer {:?}", peer_message_context.peer_id);
                        // Note: we ignore empty messages bc they are peer requests for wallet state
                        // which are handled by the frost task or are malicious/faulty requests
                        // that would cause the btc-server to wipe its state
                        if let PeerMessageResponse::WalletState(wallet_state) =
                            peer_message_context.message
                        {
                            // try parsing the uuid
                            let request_uuid = match Uuid::parse_str(&wallet_state.uuid) {
                                Ok(uuid) => uuid,
                                Err(e) => {
                                    error!(target: "consensus::authority::sync_wallet_state", ?e, "Failed to parse uuid from peer message");
                                    continue;
                                }
                            };

                            // process the wallet state
                            debug!(target: "consensus::authority::sync_wallet_state", "Received wallet state from peer {:?}", wallet_state);

                            // process the finalized pegout ids
                            let finalized_pegout_ids_compressed = wallet_state.finalized_pegout_ids;
                            let finalized_pegout_ids_decompressed = {
                                if finalized_pegout_ids_compressed.is_empty() {
                                    warn!(target: "consensus::authority::sync_wallet_state", "Peer sent empty finalized pegout ids");
                                    continue;
                                } else {
                                    let Ok(finalized_pegout_ids_decompressed) = data_parser.decompress(&finalized_pegout_ids_compressed).await.map_err(|e| {
                                        error!(target: "consensus::authority::sync_wallet_state", "Failed to decompress finalized pegout ids {:?}", e);
                                        WalletStateSyncError::CompressorError(e)
                                    }) else {
                                        tracing::error!(target: "consensus::authority::sync_wallet_state", "Failed to decompress finalized pegout ids");
                                        continue;
                                    };
                                    let Ok(finalized_pegout_ids_decompressed) = ProstMessageSerdelizer::<GetFinalizedPegoutIdsResponse>::deserialize(
                                        finalized_pegout_ids_decompressed,
                                    ) else {
                                        error!(target: "consensus::authority::sync_wallet_state", "Failed to deserialize pending pegouts");
                                        continue;
                                    };
                                    finalized_pegout_ids_decompressed
                                }
                            };

                            // update the sync responses map with the received wallet state (considering only good states towards liveness)
                            let mut current_response_cycle = current_response_cycle.write().await;
                            match current_response_cycle.as_mut() {
                                Some((uuid, peers)) => {
                                    if *uuid != request_uuid {
                                        warn!(target: "consensus::authority::sync_wallet_state", "Received wallet state with different uuid, ignoring");
                                        continue;
                                    }

                                    match peers.get_mut(&peer_message_context.peer_id) {
                                        Some(wallet_state_sync_peer_response) => {
                                            // append the peer response to the current response cycle
                                            wallet_state_sync_peer_response.append_received_data(&finalized_pegout_ids_decompressed.ids);
                                            wallet_state_sync_peer_response.set_chunks_expected(finalized_pegout_ids_decompressed.total_chunks);
                                            wallet_state_sync_peer_response.add_chunk_received(finalized_pegout_ids_decompressed.chunk_index);
                                        },
                                        None => {
                                            // add the peer to the response cycle
                                            let mut new_wallet_state_sync_peer_response = WalletStateSyncPeerResponse::default();
                                            new_wallet_state_sync_peer_response.set_chunks_expected(finalized_pegout_ids_decompressed.total_chunks);
                                            new_wallet_state_sync_peer_response.add_chunk_received(finalized_pegout_ids_decompressed.chunk_index);
                                            peers.insert(peer_message_context.peer_id, new_wallet_state_sync_peer_response);
                                        }
                                    }

                                    let (fully_synced_peers_iter, fully_synced_count) = get_fully_synced_peers(&peers);
                                    if fully_synced_count as u64 >= frost_config.min_signers as u64 {
                                        // consenses the finalized pegout ids
                                        let mut condensed_finalized_pegout_ids = HashSet::new();
                                        for (_, peer_wallet_state_sync_response) in fully_synced_peers_iter {
                                            let peer_finalized_ids = peer_wallet_state_sync_response.data.iter()
                                            .cloned()
                                            .collect::<HashSet<_>>();
                                            condensed_finalized_pegout_ids.extend(peer_finalized_ids);
                                        }

                                        // Report to btc server to resync the wallet state
                                        if let Err(e) = btc_server
                                            .reset_wallet_state(ResetWalletStateRequest {
                                                finalized_pegout_ids: condensed_finalized_pegout_ids.into_iter().collect(),
                                            })
                                            .await {
                                                error!(target: "consensus::authority::sync_wallet_state", ?e, "Failed to reset wallet state");
                                                continue;
                                            }
                                    }
                                },
                                None => {
                                    warn!(target: "consensus::authority::sync_wallet_state", "No current response cycle, ignoring wallet state");
                                    continue;
                                }
                            }
                        }
                    }
                    None => {
                        warn!(target: "consensus::authority::sync_wallet_state", "Closed channels for peer messages, no wallet state received");
                        break;
                    }
                }
            }
        });

        let current_response_cycle = self.current_response_cycle.clone();
        while let Ok(canon_event) = canon_events.recv().await {
            debug!(target: "consensus::authority::snapshot_manager::run", "received canon event {:?}", canon_event);
            match canon_event {
                CanonStateNotification::Commit { new, .. } => {
                    let tip = new.tip();
                    if !tip.is_poa_epoch() || tip.header().number == 0 {
                        continue;
                    }
                    // Request the wallet state from all peers for poa epoch blocks only
                    let uuid = uuid::Uuid::new_v4();
                    if let Err(e) = self
                        .to_frost_manager
                        .send_command(FrostCommand::GetWalletStateFromPeer(uuid))
                    {
                        error!(target: "consensus::authority::sync_wallet_state", ?e, "Failed to send get wallet state command to frost manager");
                    }
                    // start the current response cycle
                    current_response_cycle.write().await.replace((uuid, HashMap::new()));
                }
                CanonStateNotification::Reorg { old: _old, new: _new } => {
                    warn!(target: "consensus::authority::snapshot_manager::run", "reorg detected, this should not happen");
                    continue;
                }
            }
        }

        Ok(())
    }
}
