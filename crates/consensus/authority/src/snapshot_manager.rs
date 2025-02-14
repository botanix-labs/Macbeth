//! Snapshot manager is responsible for persisting snapshot chunks to disk
use crate::{snapshot_tracker::ParallelSnapshots, Storage};
use comet_bft_rpc::{Client, CometBftRpcFactory, HttpCometBFTRpcClientFactory};
use futures::StreamExt;
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_data_parser::{DataParser, Error as DataParserError};
use reth_db::{
    models::{ChunkId, Snapshot, SnapshotId},
    DatabaseEnv,
};
use reth_evm::execute::BlockExecutorProvider;
use reth_primitives::{BlockNumber, BlockWithSenders};
use reth_provider::{
    BlockReaderIdExt, CanonStateNotification, CanonStateSubscriptions, ProviderError,
    ProviderFactory, SnapshotReader, SnapshotWriter, TransactionVariant,
};
use reth_rpc_types::BlockId;
use std::sync::{Arc, RwLock};
use tracing::{debug, error, info, trace, warn};

// TODO(armins) this is defined in reth-node-core, we should move it to reth-consensus-authority but
// there is a circular dependency
/// Snapshot message format for state sync prod
pub const SNAPSHOT_MESSAGE_FORMAT: u32 = 1;

/// Snapshot message format for state sync test
pub const SNAPSHOT_MESSAGE_FORMAT_TEST: u32 = 2;

/// Snapshot size limits for state sync
pub struct SnapshotSizeLimits {
    /// Maximum snapshot size in bytes
    pub snapshot_max_size: usize,
    /// Maximum snapshot chunk size in bytes
    pub snapshot_chunk_size: usize,
}

/// Snapshot size limits for state sync test
pub const SNAPSHOT_SIZE_LIMITS_TEST: SnapshotSizeLimits = SnapshotSizeLimits {
    snapshot_max_size: 1024 * 5, // 5 KB
    snapshot_chunk_size: 1024,   // 1 KB
};

/// Snapshot size limits for state sync prod
pub const SNAPSHOT_SIZE_LIMITS_PROD: SnapshotSizeLimits = SnapshotSizeLimits {
    snapshot_max_size: 1024 * 1024 * 500,  // 500 MB
    snapshot_chunk_size: 1024 * 1024 * 10, // 10 MB
};

/// Snapshot Manager State Lock
#[derive(Clone, Debug, Default)]
pub struct SnapshotManagerStateLock {
    snapshot_id: SnapshotId,
    block_id: BlockNumber,
    is_syncing_history: bool,
}

impl SnapshotManagerStateLock {
    /// Set snapshot id
    pub fn set_snapshot_id(&mut self, snapshot_id: SnapshotId) -> &mut Self {
        self.snapshot_id = snapshot_id;
        self
    }

    /// Set block number
    pub fn set_block_number(&mut self, block_id: BlockNumber) -> &mut Self {
        self.block_id = block_id;
        self
    }

    /// Set historical sync
    pub fn set_is_syncing_history(&mut self, is_syncing_history: bool) -> &mut Self {
        self.is_syncing_history = is_syncing_history;
        self
    }

    /// Get snapshot id
    pub fn get_snapshot_id(&self) -> u64 {
        self.snapshot_id
    }

    /// Get block id
    pub fn get_block_id(&self) -> u64 {
        self.block_id
    }

    /// Check if syncing historically
    pub fn is_syncing_history(&self) -> bool {
        self.is_syncing_history
    }
}

/// Snapshot manager error
#[derive(Debug, thiserror::Error)]
pub enum SnapshotManagerError {
    #[error("db provider error: {0}")]
    /// Error related to the database provider
    Provider(#[from] ProviderError),
    /// Error related to the data parser
    #[error("Data parser error: {0}")]
    DataParser(#[from] DataParserError),
    /// Tendermint error
    #[error("Tendermint rpc error: {0}")]
    Tendermint(tendermint_rpc::Error),
}

/// Snapshot manager monitoring trait
pub trait SnapshotRunnable {
    /// Starts the snapshot runnerable
    fn run(&mut self)
        -> impl std::future::Future<Output = Result<(), SnapshotManagerError>> + Send;
}

/// Snapshot manager is responsible for persisting snapshot chunks to disk
#[allow(dead_code)]
pub struct SnapshotManager<EF, BF, DB> {
    storage: Storage<EF, BF, DB>,
    compressor: DataParser,
    provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
    snapshots_to_keep: u64,
    snapshot_message_format: u32,
    state_lock: Arc<RwLock<SnapshotManagerStateLock>>,
    snapshot_size_limits: SnapshotSizeLimits,
    enable_historical_sync: bool,
    cometbft_rpc_factory: HttpCometBFTRpcClientFactory,
}

impl<EF, BF, DB> SnapshotManager<EF, BF, DB>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    DB: BlockReaderIdExt
        + SnapshotWriter
        + SnapshotReader
        + CanonStateSubscriptions
        + Clone
        + 'static,
{
    /// Constructor
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        storage: Storage<EF, BF, DB>,
        compressor: DataParser,
        provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
        snapshots_to_keep: u64,
        snapshot_message_format: u32,
        enable_historical_sync: bool,
        state_lock: Arc<RwLock<SnapshotManagerStateLock>>,
        cometbft_rpc_factory: HttpCometBFTRpcClientFactory,
    ) -> Self {
        let snapshot_size_limits = match snapshot_message_format {
            SNAPSHOT_MESSAGE_FORMAT => SNAPSHOT_SIZE_LIMITS_PROD,
            SNAPSHOT_MESSAGE_FORMAT_TEST => SNAPSHOT_SIZE_LIMITS_TEST,
            _ => SNAPSHOT_SIZE_LIMITS_PROD,
        };
        Self {
            storage,
            compressor,
            provider_factory,
            snapshots_to_keep,
            snapshot_message_format,
            state_lock,
            snapshot_size_limits,
            enable_historical_sync,
            cometbft_rpc_factory,
        }
    }

    /// Create a new snapshot
    fn create_new_snapshot(
        &self,
        sealed_block: &BlockWithSenders,
    ) -> Result<SnapshotId, SnapshotManagerError> {
        let provider_rw = self.provider_factory.provider_rw()?;
        let snapshot_id = provider_rw
            .create_new_snapshot(sealed_block.number, sealed_block.header.hash_slow())?;
        provider_rw.commit()?;
        Ok(snapshot_id)
    }

    /// Remove oldest snapshot
    fn remove_oldest_snapshot(&self) -> Result<(), SnapshotManagerError> {
        let provider_rw = self.provider_factory.provider_rw()?;
        provider_rw.remove_oldest_snapshot()?;
        provider_rw.commit()?;
        Ok(())
    }

    /// Create a new chunk
    fn create_new_chunk(
        &self,
        snapshot_id: SnapshotId,
        block_id: BlockNumber,
        chunk_data: Vec<u8>,
    ) -> Result<ChunkId, SnapshotManagerError> {
        let provider_rw = self.provider_factory.provider_rw()?;
        let chunk_id = provider_rw.create_new_chunk(snapshot_id, block_id, chunk_data)?;
        provider_rw.commit()?;
        Ok(chunk_id)
    }

    /// Insert block snapshot id mapping
    fn insert_block_snapshot_id_mapping(
        &self,
        block_id: BlockNumber,
        snapshot_id: SnapshotId,
    ) -> Result<(), SnapshotManagerError> {
        let provider_rw = self.provider_factory.provider_rw()?;
        provider_rw.insert_block_snapshot_id_mapping(block_id, snapshot_id)?;
        provider_rw.commit()?;
        Ok(())
    }

    /// Get snapshot
    fn update_snapshot(
        &self,
        snapshot_id: SnapshotId,
        block_id: BlockNumber,
        chunk_id: ChunkId,
    ) -> Result<(), SnapshotManagerError> {
        let provider_rw = self.provider_factory.provider_rw()?;
        provider_rw.update_snapshot(snapshot_id, block_id, chunk_id)?;
        provider_rw.commit()?;
        Ok(())
    }

    /// Get snapshots size
    fn get_snapshot_size(
        &self,
        last_snapshot_id: SnapshotId,
    ) -> Result<usize, SnapshotManagerError> {
        Ok(self.provider_factory.provider()?.get_snapshot_size(last_snapshot_id)?)
    }

    /// Get snapshots count
    fn get_snapshots_count(&self) -> Result<usize, SnapshotManagerError> {
        Ok(self.provider_factory.provider()?.get_snapshots_count()?)
    }

    /// Get last snapshot height
    fn get_last_snapshot_height(
        &self,
    ) -> Result<Option<(SnapshotId, BlockNumber)>, SnapshotManagerError> {
        Ok(self.provider_factory.provider()?.get_last_snapshot_height()?)
    }

    /// Get oldest persisted block height
    fn get_oldest_persisted_block_height(
        &self,
    ) -> Result<Option<BlockNumber>, SnapshotManagerError> {
        if let Some((snapshot_id, _block_number)) =
            self.provider_factory.provider()?.get_first_snapshot_height()?
        {
            if let Some(oldest_snapshot_chunk_id) = self
                .provider_factory
                .provider()?
                .get_snapshot_by_id(snapshot_id)?
                .and_then(|s| s.get_oldest_chunk_id())
            {
                return self
                    .provider_factory
                    .provider()?
                    .get_chunk_by_id(oldest_snapshot_chunk_id)
                    .map(|sc| sc.map(|c| c.get_starting_block_number()))
                    .map_err(SnapshotManagerError::Provider);
            }
        }
        Ok(None)
    }

    #[allow(dead_code)]
    fn get_snapshot_by_id(
        &self,
        snapshot_id: SnapshotId,
    ) -> Result<Option<Snapshot>, SnapshotManagerError> {
        Ok(self.provider_factory.provider()?.get_snapshot_by_id(snapshot_id)?)
    }

    fn append_to_chunk(
        &self,
        chunk_id: ChunkId,
        block_number: BlockNumber,
        data: Vec<u8>,
    ) -> Result<(), SnapshotManagerError> {
        Ok(self.provider_factory.provider_rw()?.append_to_chunk(chunk_id, block_number, data)?)
    }
}

impl<EF, BF, DB> SnapshotRunnable for SnapshotManager<EF, BF, DB>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    DB: BlockReaderIdExt
        + SnapshotWriter
        + SnapshotReader
        + CanonStateSubscriptions
        + Clone
        + 'static,
{
    async fn run(&mut self) -> Result<(), SnapshotManagerError> {
        trace!(target: "consensus::authority::snapshot_manager::run", "started");
        trace!(target: "consensus::authority::snapshot_manager::run", "historical syncing enabled: {}", self.enable_historical_sync);

        // setup clients
        let block_client = Arc::new(self.storage.client.clone());
        let comet_rpc_client = match self.cometbft_rpc_factory.build_and_connect() {
            Ok(client) => client,
            Err(e) => {
                error!(target: "consensus::authority::snapshot_manager::run", "Failed to connect to comet light client {:?}", e);
                return Err(SnapshotManagerError::Tendermint(e));
            }
        };

        let latest_block_height: u64 = comet_rpc_client
            .latest_block()
            .await
            .ok()
            .map(|b| {
                error!(target: "consensus::authority::snapshot_manager::run", "Failed to get latest block from cometbft {:?}", b);
                b.block.header.height.into()
            })
            .unwrap_or_default();
        info!(target: "consensus::authority::snapshot_manager::run", "Latest comet block height {:?}", latest_block_height);

        // get the oldest persisted snapshot block height
        let oldest_persisted_block_height =
            self.get_oldest_persisted_block_height()?.unwrap_or_default();

        // process historical blocks if needed
        let missing_blocks = oldest_persisted_block_height;

        // create the historical blocks stream
        let (mut historical_blocks_stream, mut snapshot_tracker) = match missing_blocks {
            val if val == 0 && self.enable_historical_sync => {
                info!(target: "consensus::authority::snapshot_manager::run", "No missing blocks, starting live sync");
                let (id, size) = match self.get_last_snapshot_height()? {
                    Some((id, _)) => (id, self.get_snapshot_size(id)?),
                    None => (0, 0),
                };
                let snapshot_tracker = ParallelSnapshots::new(id, size, None);
                // mark the state lock as not syncing history, just live blocks
                let mut state_lock = self.state_lock.write().expect("snapshot state sync locked");
                state_lock.set_is_syncing_history(false);
                drop(state_lock);
                (futures::stream::empty::<Option<BlockWithSenders>>().boxed(), snapshot_tracker)
            }
            val if val > 0 && self.enable_historical_sync => {
                info!(target: "consensus::authority::snapshot_manager::run", "Missing blocks detected, starting historical sync");
                // mark the state lock as syncing history
                let mut state_lock = self.state_lock.write().expect("snapshot state sync locked");
                state_lock.set_is_syncing_history(true);
                drop(state_lock);

                // create a stream of missing blocks
                let historical_stream = futures::stream::iter(0..=missing_blocks)
                    .map(|block_number| {
                        block_client
                            .block_with_senders_by_id(
                                BlockId::Number(block_number.into()),
                                TransactionVariant::WithHash,
                            )
                            .ok()
                            .flatten()
                    })
                    .boxed();

                let (id, size) = match self.get_last_snapshot_height()? {
                    Some((id, _)) => (id, self.get_snapshot_size(id)?),
                    None => (0, 0),
                };
                let snapshot_tracker =
                    ParallelSnapshots::new(id, size, Some((0, oldest_persisted_block_height)));
                (historical_stream, snapshot_tracker)
            }
            _ => {
                warn!(target: "consensus::authority::snapshot_manager::run", "Missing blocks detected but historical sync is disabled !");
                return Ok(())
            }
        };

        // start the multiplexing loop
        let mut canon_events = self.storage.client.subscribe_to_canonical_state();
        loop {
            tokio::select! {
                historical_block = historical_blocks_stream.next() => {
                    match historical_block {
                        Some(Some(historical_block_with_senders)) => {
                            debug!(target: "consensus::authority::snapshot_manager::run", "Received block number {} with senders from historical stream", historical_block_with_senders.number);

                            if let Err(e) = self.process_block(&historical_block_with_senders, &mut snapshot_tracker).await {
                                error!(target: "consensus::authority::snapshot_manager::run",
                                      "Failed to process historical block {:?}: {:?}", historical_block_with_senders.number, e);
                                continue;
                            }

                            if historical_block_with_senders.number % 1000 == 0 {
                                debug!(target: "consensus::authority::snapshot_manager::run",
                                      "Historical sync progress: {:?}. {:?}",
                                      historical_block_with_senders.number,
                                      snapshot_tracker.get_progress_info());
                            }
                        }
                        None => {
                            // Stream is complete
                            info!(target: "consensus::authority::snapshot_manager::run", "Historical sync completed. {}", snapshot_tracker.get_progress_info());

                            // Update sync status
                            let mut state_lock = self.state_lock.write().expect("snapshot state sync locked");
                            state_lock.set_is_syncing_history(false);
                            drop(state_lock);
                        }
                        Some(None) => {
                            // Block not found or error getting block
                            warn!(target: "consensus::authority::snapshot_manager::run", "Failed to get historical block");
                            continue;
                        }
                    }
                }
                canon_event = canon_events.recv() => {
                    if let Ok(canon_event) = canon_event {
                        debug!(target: "consensus::authority::snapshot_manager::run", "received canon event {:?}", canon_event);
                        match canon_event {
                            CanonStateNotification::Commit { new, .. } => {
                                let block_with_senders = new.first().clone().unseal();

                                debug!(target: "consensus::authority::snapshot_manager::run", "Processing live block {:?}", block_with_senders.number);

                                if let Err(e) = self.process_block(&block_with_senders, &mut snapshot_tracker).await {
                                    error!(target: "consensus::authority::snapshot_manager::run",
                                          "Failed to process live block {}: {:?}", block_with_senders.number, e);
                                    continue;
                                }

                                if block_with_senders.number % 1000 == 0 {
                                    info!(target: "consensus::authority::snapshot_manager::run",
                                          "Live sync progress: {}. {}",
                                          block_with_senders.number,
                                          snapshot_tracker.get_progress_info());
                                }

                                self.apply_retention_policy()?;
                            }
                            CanonStateNotification::Reorg { old: _old, new: _new } => {
                                warn!(target: "consensus::authority::snapshot_manager::run", "reorg detected, this should not happen");
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
    }
}

impl<EF, BF, DB> SnapshotManager<EF, BF, DB>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    DB: BlockReaderIdExt
        + SnapshotWriter
        + SnapshotReader
        + CanonStateSubscriptions
        + Clone
        + 'static,
{
    /// Process a single block and update snapshots accordingly
    async fn process_block(
        &self,
        block: &BlockWithSenders,
        tracker: &mut ParallelSnapshots,
    ) -> Result<(), SnapshotManagerError> {
        // Determine if this is a historical block
        let is_historical = tracker.is_historical_block(block.number);

        // Validate block sequence
        if !tracker.validate_block_sequence(block.number, is_historical) {
            warn!(target: "consensus::authority::snapshot_manager",
                  "Block {} out of sequence for {} processing",
                  block.number,
                  if is_historical { "historical" } else { "live" });
            return Ok(());
        }

        // Serialize block
        let serialized_block = self.compressor.encode(block).await.map_err(|e| {
            error!(target: "consensus::authority::snapshot_manager",
                   "Failed to serialize block: {:?}", e);
            SnapshotManagerError::DataParser(e)
        })?;

        if serialized_block.is_empty() {
            return Ok(());
        }

        let block_size = serialized_block.len();

        // Check if we need a new snapshot
        let current_size = tracker.current_size(is_historical);
        if current_size + block_size > self.snapshot_size_limits.snapshot_max_size {
            // Create new snapshot with next sequential ID
            let snapshot_id = tracker.increment_snapshot_id(is_historical);
            let new_id = self.create_new_snapshot(block)?;

            // Verify ID sequence
            assert_eq!(
                snapshot_id, new_id,
                "Snapshot ID mismatch: expected {}, got {}",
                snapshot_id, new_id
            );

            info!(target: "consensus::authority::snapshot_manager",
                  "{} snapshot {} created for block {}",
                  if is_historical { "Historical" } else { "Live" },
                  snapshot_id,
                  block.number);

            tracker.reset_snapshot_size(is_historical);
        }

        // Get current IDs after potential snapshot creation
        let current_snapshot_id = tracker.current_snapshot_id(is_historical);

        // Check if we need a new chunk based on size limit
        let current_chunk_size = tracker.current_chunk_size(is_historical);
        let needs_new_chunk =
            current_chunk_size + block_size > self.snapshot_size_limits.snapshot_chunk_size;

        if needs_new_chunk || tracker.current_chunk_id(is_historical) == 0 {
            // Either chunk is full or we don't have a chunk yet
            let chunk_id =
                self.create_new_chunk(current_snapshot_id, block.number, serialized_block.clone())?;

            if needs_new_chunk {
                info!(target: "consensus::authority::snapshot_manager",
                        "{} chunk {} full ({:.2}MB), created new chunk {} for snapshot {}",
                        if is_historical { "Historical" } else { "Live" },
                        tracker.current_chunk_id(is_historical),
                        current_chunk_size as f64 / (1024.0 * 1024.0),
                        chunk_id,
                        current_snapshot_id);
            }

            tracker.reset_chunk_size(is_historical);
        } else {
            // Append to current chunk
            self.append_to_chunk(
                tracker.current_chunk_id(is_historical),
                block.number,
                serialized_block.clone(),
            )?;
        }

        // Update snapshot metadata
        self.update_snapshot(
            current_snapshot_id,
            block.number,
            tracker.current_chunk_id(is_historical),
        )?;
        self.insert_block_snapshot_id_mapping(block.number, current_snapshot_id)?;

        // Update sizes and block tracking
        tracker.add_size(block_size, is_historical);

        // Verify
        if let Ok(actual_size) = self.get_snapshot_size(current_snapshot_id) {
            let tracked_size = tracker.current_size(is_historical);
            if tracked_size != actual_size {
                warn!(target: "consensus::authority::snapshot_manager",
                      "{} snapshot size mismatch: tracked={:.2}MB, actual={:.2}MB",
                      if is_historical { "Historical" } else { "Live" },
                      tracked_size as f64 / (1024.0 * 1024.0),
                      actual_size as f64 / (1024.0 * 1024.0));
            }
        }

        tracker.update_last_block(block.number, is_historical);

        // Check if historical sync is complete
        if is_historical && tracker.is_syncing_history_complete() {
            info!(target: "consensus::authority::snapshot_manager",
            "Historical sync complete at block {}. {}",
            block.number,
            tracker.get_progress_info());
            tracker.complete_historical_sync();
            let mut state_lock = self.state_lock.write().expect("snapshot state sync locked");
            state_lock.set_is_syncing_history(false);
            drop(state_lock);
        }

        Ok(())
    }

    /// Apply retention policy for snapshots
    fn apply_retention_policy(&self) -> Result<(), SnapshotManagerError> {
        if !self.state_lock.read().expect("snapshot state sync locked").is_syncing_history() &&
            self.get_snapshots_count()? > self.snapshots_to_keep as usize
        {
            if let Some((_, oldest_height)) = self.provider_factory.get_first_snapshot_height()? {
                let state_lock = self.state_lock.read().expect("snapshot state sync locked");
                if state_lock.block_id != oldest_height {
                    info!(target: "consensus::authority::snapshot_manager",
                          "Removing oldest snapshot at height {}", oldest_height);
                    self.remove_oldest_snapshot()?;
                }
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::MAINNET;
    use reth_db_common::init::init_genesis;
    use reth_primitives::B256;
    use reth_provider::test_utils::create_test_provider_factory_with_chain_spec;
    use std::time::Duration;

    #[tokio::test]
    async fn test_rw_snapshots() {
        let provider_factory = create_test_provider_factory_with_chain_spec(MAINNET.clone());

        init_genesis(provider_factory.clone()).unwrap();
        let client = provider_factory.provider_rw().unwrap();

        // insert a new snapshot at block height 1
        client.create_new_snapshot(1, B256::random()).unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;

        // assertions
        let snapshots = client.get_snapshots().unwrap();
        assert!(snapshots.len() == 1);
        let first_snapshot = snapshots.first().unwrap().clone();
        assert!(first_snapshot.id() == 1);
        assert!(first_snapshot.height() == 1);
        assert!(first_snapshot.block_ids().is_empty());
        assert!(first_snapshot.chunk_ids().is_empty());
        assert!(first_snapshot.size() > 0);
        let last_snapshot = snapshots.last().unwrap().clone();
        assert!(last_snapshot.height() == 1);
        assert!(last_snapshot.block_ids().is_empty());
        assert!(last_snapshot.chunk_ids().is_empty());
        assert!(last_snapshot.size() > 0);
        let snapshots_count = client.get_snapshots_count().unwrap();
        assert!(snapshots_count == 1);
        let (snapshot_id, block_number) = client.get_first_snapshot_height().unwrap().unwrap();
        assert!(snapshot_id == 1);
        assert!(block_number == 1);
        let (snapshot_id, block_number) = client.get_last_snapshot_height().unwrap().unwrap();
        assert!(snapshot_id == 1);
        assert!(block_number == 1);

        // insert a new snapshot at block height 2
        client.create_new_snapshot(2, B256::random()).unwrap();

        // assertions
        let snapshots = client.get_snapshots().unwrap();
        assert!(snapshots.len() == 2);
        let first_snapshot = snapshots.first().unwrap().clone();
        assert!(first_snapshot.id() == 1);
        assert!(first_snapshot.height() == 1);
        assert!(first_snapshot.block_ids().is_empty());
        assert!(first_snapshot.chunk_ids().is_empty());
        let last_snapshot = snapshots.last().unwrap().clone();
        assert!(last_snapshot.id() == 2);
        assert!(last_snapshot.height() == 2);
        assert!(last_snapshot.block_ids().is_empty());
        assert!(last_snapshot.chunk_ids().is_empty());
        let snapshots_count = client.get_snapshots_count().unwrap();
        assert!(snapshots_count == 2);
        let (snapshot_id, block_number) = client.get_first_snapshot_height().unwrap().unwrap();
        assert!(snapshot_id == 1);
        assert!(block_number == 1);
        let (snapshot_id, block_number) = client.get_last_snapshot_height().unwrap().unwrap();
        assert!(snapshot_id == 2);
        assert!(block_number == 2);

        let snapshot_by_id = client.get_snapshot_by_id(snapshot_id).unwrap().unwrap();
        assert!(snapshot_by_id.height() == snapshot_id);
        assert!(snapshot_by_id.block_ids().is_empty());
        assert!(snapshot_by_id.chunk_ids().is_empty());

        let snapshot_id_by_block_id =
            client.get_snapshot_id_by_block_id(block_number).unwrap().unwrap();
        assert!(snapshot_id == snapshot_id_by_block_id);

        client.remove_oldest_snapshot().unwrap(); // should be 1
        let snapshots_count = client.get_snapshots_count().unwrap();
        assert!(snapshots_count == 1);
        let snapshots = client.get_snapshots().unwrap();
        assert!(snapshots.len() == 1);
        let _snp = snapshots.first().unwrap().clone();
        assert!(snapshot_by_id.height() == 2);

        client.remove_snapshots(2..=2).unwrap();
        let snapshots_count = client.get_snapshots_count().unwrap();
        assert!(snapshots_count == 0);
    }

    #[tokio::test]
    async fn test_rw_snapshots_with_chunks() {
        let provider_factory = create_test_provider_factory_with_chain_spec(MAINNET.clone());

        init_genesis(provider_factory.clone()).unwrap();
        let client = provider_factory.provider_rw().unwrap();

        // insert a new snapshot
        let block_id = 1;
        let snapshot_id = client.create_new_snapshot(block_id, B256::random()).unwrap();

        // insert block with some chunks
        let (first_block_id, first_block_chunks) = (1, 1..=10);
        for chunk_id in first_block_chunks.clone() {
            client.update_snapshot(snapshot_id, first_block_id, chunk_id).unwrap();
        }

        // insert another block with some chunks
        let (second_block_id, second_block_chunks) = (2, 11..=20);
        for chunk_id in second_block_chunks.clone() {
            client.update_snapshot(snapshot_id, second_block_id, chunk_id).unwrap();
        }

        // assertions
        let snapshots = client.get_snapshots().unwrap();
        assert!(snapshots.len() == 1);
        let snapshots_count = client.get_snapshots_count().unwrap();
        assert!(snapshots_count == 1);
        let snapshot = snapshots.first().unwrap().clone();
        assert!(snapshot.height() == second_block_id);
        let combined_blocks: Vec<_> = first_block_chunks.chain(second_block_chunks).collect();
        assert!(snapshot.chunk_ids() == combined_blocks.as_slice());
    }

    #[tokio::test]
    async fn test_rw_snapshots_with_duplicate_chunks() {
        let provider_factory = create_test_provider_factory_with_chain_spec(MAINNET.clone());

        init_genesis(provider_factory.clone()).unwrap();
        let client = provider_factory.provider_rw().unwrap();

        // insert a new snapshot
        let block_id = 1;
        let snapshot_id = client.create_new_snapshot(block_id, B256::random()).unwrap();

        // insert block with some chunks
        let (first_block_id, first_block_chunks) = (1, 1..=10);
        for chunk_id in first_block_chunks.clone() {
            client.update_snapshot(snapshot_id, first_block_id, chunk_id).unwrap();
        }

        // insert same block with chunks
        for chunk_id in first_block_chunks.clone() {
            client.update_snapshot(snapshot_id, first_block_id, chunk_id).unwrap();
        }

        // assertions
        let snapshots = client.get_snapshots().unwrap();
        assert!(snapshots.len() == 1);
        let snapshots_count = client.get_snapshots_count().unwrap();
        assert!(snapshots_count == 1);
        let snapshot = snapshots.first().unwrap().clone();
        assert!(snapshot.height() == first_block_id);
        let combined_blocks: Vec<_> = first_block_chunks.collect();
        assert!(snapshot.chunk_ids() == combined_blocks.as_slice());
    }

    #[tokio::test]
    async fn test_rw_snapshots_with_chunk_batches() {
        let provider_factory = create_test_provider_factory_with_chain_spec(MAINNET.clone());

        init_genesis(provider_factory.clone()).unwrap();
        let client = provider_factory.provider_rw().unwrap();

        // insert a new snapshot
        let block_id = 1;
        let snapshot_id = client.create_new_snapshot(block_id, B256::random()).unwrap();
        assert!(snapshot_id == 1);

        // insert block with some chunks
        let (first_block_id, first_block_chunks) = (1, 1..=10);
        for chunk_id in first_block_chunks.clone() {
            client.update_snapshot(snapshot_id, first_block_id, chunk_id).unwrap();
        }

        // insert another block with some chunks
        let second_block_chunks = 11..=20;
        for chunk_id in second_block_chunks.clone() {
            client.update_snapshot(snapshot_id, first_block_id, chunk_id).unwrap();
        }

        // assertions
        let snapshots = client.get_snapshots().unwrap();
        assert!(snapshots.len() == 1);
        let snapshots_count = client.get_snapshots_count().unwrap();
        assert!(snapshots_count == 1);
        let snapshot = snapshots.first().unwrap().clone();
        assert!(snapshot.height() == first_block_id);
        let combined_blocks: Vec<_> = first_block_chunks.chain(second_block_chunks).collect();
        assert!(snapshot.chunk_ids() == combined_blocks.as_slice());
    }

    #[tokio::test]
    async fn test_rw_chunks() {
        let provider_factory = create_test_provider_factory_with_chain_spec(MAINNET.clone());

        init_genesis(provider_factory.clone()).unwrap();
        let client = provider_factory.provider_rw().unwrap();

        // insert block chunks
        let snapshot_id = 1;
        let chunk_data: Vec<Vec<u8>> = vec![vec![1, 2, 3, 4, 5]];
        let block_ids = 1..=10;
        // loop over block_heights
        let mut chunk_id = 0;
        for block_id in block_ids.clone() {
            chunk_id =
                client.create_new_chunk(snapshot_id, block_id, chunk_data[0].clone()).unwrap();
        }
        assert!(chunk_id == *block_ids.end());
        assert!(client.get_last_chunk_id().unwrap().unwrap() == *block_ids.end());
        assert!(client.get_first_chunk_id().unwrap().unwrap() == *block_ids.start());

        let chunk_by_id = client.get_chunk_by_id(*block_ids.end()).unwrap().unwrap();
        assert!(chunk_by_id.snapshot_id() == snapshot_id);
        assert!(chunk_by_id.chunk_data().to_vec() == chunk_data);

        let block_id = client.get_chunk_block_number(chunk_id).unwrap().unwrap();
        assert!(block_id == *block_ids.end());
    }

    #[tokio::test]
    async fn test_rw_snapshot_syncs() {
        let provider_factory = create_test_provider_factory_with_chain_spec(MAINNET.clone());

        init_genesis(provider_factory.clone()).unwrap();
        let client = provider_factory.provider_rw().unwrap();

        // insert a new snapshot sync
        client.create_new_snapshot_sync(1, B256::random(), 10, 1).unwrap();
        client.create_new_snapshot_sync(2, B256::random(), 20, 1).unwrap();
        let id = client.create_new_snapshot_sync(3, B256::random(), 30, 1).unwrap();

        let mut snapshot_sync_by_id = client.get_snapshot_sync_by_id(id).unwrap().unwrap();
        assert!(snapshot_sync_by_id.height() == 3);
        assert!(snapshot_sync_by_id.total_chunks() == 30);
        assert!(snapshot_sync_by_id.format() == 1);
        assert!(snapshot_sync_by_id.last_applied_chunk_index() == 0);

        let last_snapshot_sync_id = client.get_last_snapshot_sync_id().unwrap().unwrap();
        assert!(last_snapshot_sync_id == 3);

        snapshot_sync_by_id.set_height(33);
        snapshot_sync_by_id.set_total_chunks(44);
        client.update_snapshot_sync(3, snapshot_sync_by_id).unwrap();

        let updated_snapshot_sync_by_id = client.get_snapshot_sync_by_id(id).unwrap().unwrap();
        assert!(updated_snapshot_sync_by_id.height() == 33);
        assert!(updated_snapshot_sync_by_id.total_chunks() == 44);
        assert!(updated_snapshot_sync_by_id.format() == 1);
        assert!(updated_snapshot_sync_by_id.last_applied_chunk_index() == 0);
    }
}
