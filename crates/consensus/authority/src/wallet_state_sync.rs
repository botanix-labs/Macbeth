//! Wallet state sync module
use crate::{
    utils::{get_block_pegouts, EpochPegoutsError},
    Storage,
};
use bitcoin::hashes::{sha256::Hash as Sha256Hash, FromSliceError};
use botanix_authority_edh::extra_data_header::ExtraDataHeaderDeserializeError;
use botanix_btc_wallet::bitcoind::BitcoindFactory;
use botanix_data_parser::{
    prost_parser::ProstMessageSerdelizer, DataParser, Error as CompressorError, SerializationType,
};
use botanix_storage::{models::uuid_to_b256, WalletStateSyncReader, WalletStateSyncWriter};
use btc_server_client::{
    BtcServerExtendedApi, FinalizedPegout, GetFinalizedPegoutIdsResponse, GrpcClientError,
    ResetWalletStateRequest,
};
use btcserverlib::pegout_id::PegoutId;
use once_cell::sync::Lazy;
use reth_evm::execute::BlockExecutorProvider;
use reth_network::frost::{
    manager::{FrostCommand, FrostConfig, ToFrostManager},
    PeerMessageResponse,
};
use reth_primitives::Bytes;
use reth_provider::{
    BlockReaderIdExt, CanonStateNotification, CanonStateSubscriptions, ProviderError,
};
use reth_tasks::TaskExecutor;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc::error::SendError, RwLock};
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

const MAX_BLOCK_TS_CUTOFF_DURATION_SECS: u64 = 30 * 24 * 60 * 60 * 3; // 3 months

/// Maximum duration for block timestamp cutoff
/// This is used to determine how far back we should consider finalized pegouts when syncing.
pub static MAX_BLOCK_TS_CUTOFF_DURATION: Lazy<Duration> =
    Lazy::new(|| Duration::from_secs(MAX_BLOCK_TS_CUTOFF_DURATION_SECS));

#[derive(Debug, thiserror::Error)]
/// Wallet state synchronization errors
pub enum WalletStateSyncError {
    #[error("db provider error: {0}")]
    /// Error related to the database provider
    Provider(#[from] ProviderError),
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

type WalletStateSyncResponseCycle = Arc<RwLock<Option<Uuid>>>;
#[derive(Clone)]
/// Engine for synchronizing wallet state
pub struct WalletStateSyncEngine<EF, BF, RDB, BDB, ToFrostMan, BtcServerClient> {
    storage: Storage<EF, BF, RDB, BDB>,
    btc_server: BtcServerClient,
    to_frost_manager: ToFrostMan,
    data_parser: DataParser,
    task_executor: TaskExecutor,
    frost_config: FrostConfig,
    current_response_cycle: WalletStateSyncResponseCycle,
}

impl<EF, BF, RDB, BDB, ToFrostMan, BtcServerClient>
    WalletStateSyncEngine<EF, BF, RDB, BDB, ToFrostMan, BtcServerClient>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    ToFrostMan: ToFrostManager + Sync + Clone + 'static,
    RDB: BlockReaderIdExt + CanonStateSubscriptions + Clone + 'static,
    BDB: WalletStateSyncWriter + WalletStateSyncReader + Clone + 'static,
    BtcServerClient: BtcServerExtendedApi + Clone,
{
    pub(crate) fn new(
        storage: Storage<EF, BF, RDB, BDB>,
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

/// check the L2 existence of the pegouts
async fn hydrate_minimum_superset(
    minimum_superset: HashSet<(u64, Bytes)>,
    client: &impl BlockReaderIdExt,
    btc_network: bitcoin::Network,
) -> Result<HashMap<u64, Vec<(PegoutId, u64)>>, EpochPegoutsError> {
    // Group data by block number
    let mut superset_map: HashMap<u64, Vec<Bytes>> = HashMap::new();
    for (block_num, data) in minimum_superset {
        superset_map.entry(block_num).or_default().push(data);
    }

    // Create futures for each block
    let futures = superset_map.into_iter().map(|(block, data)| async move {
        // Get valid pegout IDs for this block
        let pegouts_result =
            get_block_pegouts(block, client, btc_network, Some(*MAX_BLOCK_TS_CUTOFF_DURATION))
                .await;

        match pegouts_result {
            Ok(pegouts_in_block) => {
                // Filter data to only include valid pegout IDs
                let hydrated_data = data
                    .into_iter()
                    .filter_map(|item| match PegoutId::from_bytes(&item) {
                        Ok(pegout_id) => pegouts_in_block
                            .iter()
                            .find(|(block_pegout_id, _)| *block_pegout_id == pegout_id)
                            .cloned(),
                        Err(_) => None,
                    })
                    .collect::<Vec<_>>();

                Ok((block, hydrated_data))
            }
            Err(e) => Err(e),
        }
    });

    // Execute all futures in parallel
    let results = futures::future::join_all(futures).await;

    // Process results
    let mut hydrated_superset_map = HashMap::new();
    for result in results {
        match result {
            Ok((block, data)) => {
                if !data.is_empty() {
                    hydrated_superset_map.insert(block, data);
                }
            }
            Err(e) => return Err(e),
        }
    }

    Ok(hydrated_superset_map)
}

impl<EF, BF, RDB, BDB, ToFrostMan, BtcServerClient> WalletStateSync
    for WalletStateSyncEngine<EF, BF, RDB, BDB, ToFrostMan, BtcServerClient>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    ToFrostMan: ToFrostManager + Clone + Sync + 'static,
    RDB: BlockReaderIdExt + CanonStateSubscriptions + Clone + 'static,
    BDB: WalletStateSyncWriter + WalletStateSyncReader + Clone + 'static,
    BtcServerClient: BtcServerExtendedApi + Clone,
{
    // Note: this function should not be called unless we are fully synced
    async fn sync_wallet_state(&self) -> Result<(), WalletStateSyncError> {
        trace!(target: "consensus::authority::WalletStateSync::sync_wallet_state", "syncing wallet state");
        let mut btc_server = self.btc_server.clone();

        let (peer_messages_tx, peer_messages_rx) = tokio::sync::oneshot::channel();

        self.to_frost_manager
            .send_command(FrostCommand::GetPeerMessagesStream(peer_messages_tx))?;
        let mut peer_messages_rx = peer_messages_rx.await.expect("peer messages rx to be open");

        let data_parser = self.data_parser.clone();
        let frost_config = self.frost_config.clone();
        let current_response_cycle = self.current_response_cycle.clone();
        let mut canon_events = self.storage.reth_database.subscribe_to_canonical_state();
        let btc_network = self.storage.btc_network;
        let storage = self.storage.clone();
        let reth_database = storage.reth_database.clone();
        let botanix_provider_factory = storage.botanix_database_factory.clone();

        // TODO: Currently we commit after each write operation. We need to make sure that the data
        //  we commit here are consistent.

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
                            let current_response_cycle = current_response_cycle.read().await;
                            match *current_response_cycle {
                                Some(uuid) => {
                                    if uuid != request_uuid {
                                        warn!(target: "consensus::authority::sync_wallet_state", "Received wallet state with different uuid, ignoring");
                                        continue;
                                    }

                                    // update the state
                                    let state_sync_record_by_peer_id = match botanix_provider_factory.get_state_sync_record_by_peer_id(peer_message_context.peer_id) {
                                        Ok(state_sync_record_by_peer_id) => state_sync_record_by_peer_id,
                                        Err(e) => {
                                            error!(target: "consensus::authority::sync_wallet_state", ?e, "Failed to get state sync record by peer id");
                                            continue;
                                        }
                                    };

                                    match state_sync_record_by_peer_id.as_ref() {
                                        Some(wallet_state_sync_record) => {
                                            // check if the peer is already in the db
                                            if wallet_state_sync_record.get_uuid() != uuid_to_b256(uuid) {
                                                warn!(target: "consensus::authority::sync_wallet_state", "Peer sent different uuid, ignoring");
                                                continue;
                                            }

                                            // append the data to the state sync record
                                            match botanix_provider_factory.append_data_to_state_sync_record(
                                                wallet_state_sync_record.get_peer_id(),
                                                finalized_pegout_ids_decompressed.data.into_iter().map(|pid | (pid.botanix_block_height, Bytes::from(pid.id))).collect::<Vec<_>>(),
                                            ) {
                                                Ok(_) => {
                                                    info!(target: "consensus::authority::sync_wallet_state", "Appended data to state sync record");
                                                }
                                                Err(e) => {
                                                    error!(target: "consensus::authority::sync_wallet_state", ?e, "Failed to append data to state sync record");
                                                    continue;
                                                }
                                            }
                                        }
                                        None => {
                                            // create a new state sync record for the peer
                                            match botanix_provider_factory.create_new_state_sync_record(
                                                uuid_to_b256(uuid),
                                                peer_message_context.peer_id,
                                                finalized_pegout_ids_decompressed.total_chunks,
                                                Some(finalized_pegout_ids_decompressed.data.into_iter().map(|pid| (pid.botanix_block_height, Bytes::from(pid.id))).collect::<Vec<_>>()),
                                            ) {
                                                Ok(_) => {
                                                    info!(target: "consensus::authority::sync_wallet_state", "Created new state sync record");
                                                }
                                                Err(e) => {
                                                    error!(target: "consensus::authority::sync_wallet_state", ?e, "Failed to create new state sync record");
                                                    continue;
                                                }
                                            }
                                        }
                                    }

                                    // check if we have all the chunks and a minimum superset available
                                    match botanix_provider_factory.get_minimum_superset(frost_config.min_signers as u64) {
                                        Ok((found, minimum_superset)) => {
                                            if found {
                                                // hydrate the superset
                                                let hydrated_minimum_superset = match hydrate_minimum_superset(
                                                    minimum_superset,
                                                    &reth_database,
                                                    btc_network,
                                                ).await {
                                                    Ok(hydrated_minimum_superset) => hydrated_minimum_superset,
                                                    Err(e) => {
                                                        error!(target: "consensus::authority::sync_wallet_state", ?e, "Failed to hydrate minimum superset");
                                                        continue;
                                                    }
                                                };
                                                // prepare the grpc response
                                                let finalized_pegout_ids = hydrated_minimum_superset
                                                .into_iter()
                                                .flat_map(|(block, data)| {
                                                    data.into_iter().map(move |(pegout_id, timestamp)| {
                                                        FinalizedPegout {
                                                            botanix_block_height: block,
                                                            id: pegout_id.as_bytes().to_vec(),
                                                            botanix_block_timestamp: timestamp,
                                                        }
                                                    })
                                                })
                                                .collect::<Vec<_>>();

                                                info!(target: "consensus::authority::sync_wallet_state", "Found minimum superset, notifying frost manager");
                                                // Report to btc server to resync the wallet state
                                                match btc_server
                                                    .reset_wallet_state(ResetWalletStateRequest {
                                                        finalized_pegout_ids,
                                                    })
                                                    .await {
                                                    Ok(_) => {
                                                        info!(target: "consensus::authority::sync_wallet_state", "Wallet state reset successfully");
                                                    }
                                                    Err(e) => {
                                                        error!(target: "consensus::authority::sync_wallet_state", ?e, "Failed to reset wallet state");
                                                    }
                                                }
                                                // Remove from the db all state sync records
                                                match botanix_provider_factory.remove_all_state_sync_records() {
                                                    Ok(_) => {
                                                        info!(target: "consensus::authority::sync_wallet_state", "Removed all state sync records");
                                                    }
                                                    Err(e) => {
                                                        error!(target: "consensus::authority::sync_wallet_state", ?e, "Failed to remove all state sync records");
                                                    }
                                                }
                                            } else {
                                                warn!(target: "consensus::authority::sync_wallet_state", "Minimum superset not found yet");
                                            }
                                        }
                                        Err(e) => {
                                            error!(target: "consensus::authority::sync_wallet_state", ?e, "Failed to get minimum superset");
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
                    // (re-)start the current response cycle
                    current_response_cycle.write().await.replace(uuid);
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
