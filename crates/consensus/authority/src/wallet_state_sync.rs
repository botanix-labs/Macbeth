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

type WalletStateSyncResponseCycle = Arc<RwLock<Option<(Uuid, HashMap<PeerId, Vec<Vec<u8>>>)>>>;

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
                            let finalized_pegout_ids = {
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
                                    finalized_pegout_ids_decompressed.ids
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
                                        Some(peer_response_cycle_data) => {
                                            // append the peer response to the current response cycle
                                            peer_response_cycle_data.extend_from_slice(&finalized_pegout_ids);
                                        },
                                        None => {
                                            // add the peer to the response cycle
                                            peers.insert(peer_message_context.peer_id, vec![]);
                                        }
                                    }

                                    if peers.len() as u16 >= frost_config.min_signers {
                                        // consenses the finalized pegout ids
                                        let mut condensed_finalized_pegout_ids = HashSet::new();
                                        for peer_finalized_peer_ids in peers.values_mut() {
                                            let peer_finalized_ids = peer_finalized_peer_ids.iter()
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
