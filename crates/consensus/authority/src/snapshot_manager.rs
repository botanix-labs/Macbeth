//! Snapshot manager is responsible for persisting snapshot chunks to disk
use crate::Storage;
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
use std::{
    sync::{Arc, RwLock},
    time::Duration,
};
use tendermint_rpc::HttpClient;
use tokio::time::sleep;
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
    snapshot_max_size: 1024 * 1024 * 15,  // ~15 MB
    snapshot_chunk_size: 1024 * 1024 * 3, // ~3 MB
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

    /// Get snapshot id
    pub fn get_snapshot_id(&self) -> u64 {
        self.snapshot_id
    }

    /// Get block id
    pub fn get_block_id(&self) -> u64 {
        self.block_id
    }

    /// Set historical sync
    pub fn set_is_syncing_history(&mut self, is_syncing_history: bool) -> &mut Self {
        self.is_syncing_history = is_syncing_history;
        self
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
    /// Invalid block height for snapshot
    #[error("Invalid block height for snapshot")]
    InvalidBlockHeightForSnapshot(),
    /// Tendermint error
    #[error("Tendermint rpc error: {0}")]
    Tendermint(tendermint_rpc::Error),
}

/// Snapshot manager monitoring trait
pub trait SnapshotRunnable {
    /// Starts the snapshot runnerable
    fn run(
        &mut self,
        snapshot_signal_tx: tokio::sync::broadcast::Sender<()>,
    ) -> impl std::future::Future<Output = Result<(), SnapshotManagerError>> + Send;
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
    enable_state_sync: bool,
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
        enable_state_sync: bool,
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
            enable_state_sync,
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

    /// Get latest persisted block height
    fn get_latest_persisted_block_height(
        &self,
    ) -> Result<Option<BlockNumber>, SnapshotManagerError> {
        if let Some((_snapshot_id, block_number)) =
            self.provider_factory.provider()?.get_last_snapshot_height()?
        {
            return Ok(Some(block_number));
        }
        Ok(None)
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
    async fn run(
        &mut self,
        snapshot_signal_tx: tokio::sync::broadcast::Sender<()>,
    ) -> Result<(), SnapshotManagerError> {
        if !self.enable_state_sync {
            tracing::info!("Snapshot manager is disabled. Exiting...");
            return Ok(());
        }
        trace!(target: "consensus::authority::snapshot_manager::run", "started");

        // setup clients
        let block_client = Arc::new(self.storage.client.clone());
        let comet_rpc_client = match self.cometbft_rpc_factory.build_and_connect() {
            Ok(client) => client,
            Err(e) => {
                error!(target: "consensus::authority::snapshot_manager::run", "Failed to connect to comet light client {:?}", e);
                return Err(SnapshotManagerError::Tendermint(e));
            }
        };

        let latest_block_height = get_latest_comet_block_height(&comet_rpc_client).await;
        info!(target: "consensus::authority::snapshot_manager::run", "Latest comet block height {:?}", latest_block_height);

        // get the latest persisted snapshot block height
        let latest_persisted_block_height =
            self.get_latest_persisted_block_height()?.unwrap_or_default();
        info!(target: "consensus::authority::snapshot_manager::run", "Latest persisted block height {}", latest_persisted_block_height);

        // start from the next block after the latest persisted block height
        let starting_block = latest_persisted_block_height + 1;
        let missing_blocks = latest_block_height.saturating_sub(starting_block);
        info!(target: "consensus::authority::snapshot_manager::run", "Missing blocks {}", missing_blocks);

        // create the historical blocks stream
        let mut historical_blocks_stream = match missing_blocks {
            0 => futures::stream::empty::<Option<BlockWithSenders>>().boxed(),
            val if val > 0 && self.enable_historical_sync => {
                info!(target: "consensus::authority::snapshot_manager::run", "Missing blocks detected, starting historical sync");
                // mark the state lock as syncing history
                let mut state_lock = self.state_lock.write().expect("snapshot state sync locked");
                state_lock.set_is_syncing_history(true);
                drop(state_lock);

                // create a stream of missing blocks
                let historical_stream = futures::stream::iter(starting_block..=latest_block_height)
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

                historical_stream
            }
            _ => {
                warn!(target: "consensus::authority::snapshot_manager::run", "Missing blocks detected but historical sync is disabled!");

                let mut state_lock = self.state_lock.write().expect("snapshot state sync locked");
                state_lock.set_is_syncing_history(false);
                drop(state_lock);

                // allow a gap in snapshots and start syncing live blocks by returning an empty
                // stream
                futures::stream::empty::<Option<BlockWithSenders>>().boxed()
            }
        };

        // signal to the abci client that it is ready to start now
        if snapshot_signal_tx.send(()).is_err() {
            error!(target: "reth::cli", "Failed to send start signal to ABCI server");
        }

        if missing_blocks > 0 {
            info!(target: "consensus::authority::snapshot_manager::run", "Starting historical sync from block {} to block {}", starting_block, latest_block_height);
            loop {
                match historical_blocks_stream.next().await {
                    Some(Some(historical_block_with_senders)) => {
                        info!(target: "consensus::authority::snapshot_manager::run", "Received block number {} with senders from historical stream", historical_block_with_senders.number);

                        if let Err(e) = self.process_block(&historical_block_with_senders).await {
                            error!(target: "consensus::authority::snapshot_manager::run",
                                      "Failed to process historical block {:?}: {:?}", historical_block_with_senders.number, e);
                            continue;
                        }
                    }
                    Some(None) => {
                        info!(target: "consensus::authority::snapshot_manager::run", "No block found in historical stream");
                        continue;
                    }
                    _ => {
                        // Stream is complete
                        info!(target: "consensus::authority::snapshot_manager::run", "Historical sync completed.");
                        let mut state_lock =
                            self.state_lock.write().expect("snapshot state sync locked");
                        state_lock.set_is_syncing_history(false);
                        drop(state_lock);
                        break;
                    }
                }
            }
        }

        info!(target: "consensus::authority::snapshot_manager::run", "Starting live sync");
        let mut canon_events = self.storage.client.subscribe_to_canonical_state();
        while let Ok(canon_event) = canon_events.recv().await {
            debug!(target: "consensus::authority::snapshot_manager::run", "received canon event {:?}", canon_event);

            match canon_event {
                CanonStateNotification::Commit { new, .. } => {
                    let block_with_senders = new.first().clone().unseal();
                    debug!(target: "consensus::authority::snapshot_manager::run", "Processing live block {:?}", block_with_senders.number);

                    if let Err(e) = self.process_block(&block_with_senders).await {
                        error!(target: "consensus::authority::snapshot_manager::run",
                                          "Failed to process live block {}: {:?}", block_with_senders.number, e);
                        continue;
                    }

                    self.apply_retention_policy()?;
                }
                CanonStateNotification::Reorg { old: _old, new: _new } => {
                    warn!(target: "consensus::authority::snapshot_manager::run", "reorg detected, this should not happen");
                    return Ok(());
                }
            }
        }

        Ok(())
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
    async fn process_block(&self, block: &BlockWithSenders) -> Result<(), SnapshotManagerError> {
        // Serialize block
        let serialized_block = self.compressor.encode(block).await.map_err(|e| {
            error!(target: "consensus::authority::snapshot_manager",
                   "Failed to serialize block: {:?}", e);
            SnapshotManagerError::DataParser(e)
        })?;

        if serialized_block.is_empty() {
            return Ok(());
        }

        // check the block height vs. the last snapshot height
        let mut state_lock = self.state_lock.write().expect("snapshot state sync locked");
        let mut last_snapshot_id = match self.get_last_snapshot_height()? {
            Some((last_snapshot_id, last_snapshot_height)) => {
                if !state_lock.is_syncing_history() && block.number < last_snapshot_height {
                    error!(target: "consensus::authority::snapshot_manager::run", "block number {} is less than last snapshot height {}", block.number, last_snapshot_height);
                    return Err(SnapshotManagerError::InvalidBlockHeightForSnapshot());
                }
                last_snapshot_id
            }
            None => {
                info!(target: "consensus::authority::snapshot_manager::run", "no last snapshot height. Creating a new snapshot at height {}...", block.number); // create a new snapshot
                self.create_new_snapshot(block)?
            }
        };
        info!("Last_snapshot_id: {:?}", last_snapshot_id);

        // now check the latest snapshot size
        let latest_snapshot_size = self.get_snapshot_size(last_snapshot_id)?;
        info!(
            "Latest_snapshot_size. Serialized block size: {:?}, Latest snapshot size: {:?}",
            serialized_block.len(),
            latest_snapshot_size
        );

        // Check if there is enough space in the latest snapshot
        debug!(target: "consensus::authority::snapshot_manager::run", "Snapshot size: {}", latest_snapshot_size);
        if latest_snapshot_size + serialized_block.len() >
            self.snapshot_size_limits.snapshot_max_size
        {
            info!(target: "consensus::authority::snapshot_manager::run", "Snapshot size exceeds limit of {} bytes. Current size: {}, Attempted: {}", self.snapshot_size_limits.snapshot_max_size, latest_snapshot_size, serialized_block.len());
            // create a new snapshot
            last_snapshot_id = self.create_new_snapshot(block)?;
            info!("Created last_snapshot_id: {:?}", last_snapshot_id);
        }
        info!("Snapshots count: {:?}", self.get_snapshots_count()?);

        // update the snapshot state lock
        state_lock.set_snapshot_id(last_snapshot_id).set_block_number(block.number);
        drop(state_lock);

        let snapshot = self.get_snapshot_by_id(last_snapshot_id)?.expect("checked above");
        let chunk_id = match snapshot.get_latest_chunk_id() {
            Some(chunk_id) => {
                // Check if there is enough space in the latest chunk
                let latest_chunk_size =
                    self.provider_factory.provider()?.get_chunk_size(chunk_id)?;
                if latest_chunk_size + serialized_block.len() >
                    self.snapshot_size_limits.snapshot_chunk_size
                {
                    self.create_new_chunk(last_snapshot_id, block.number, serialized_block.clone())?
                } else {
                    // Existing chunk lets append to it
                    self.append_to_chunk(chunk_id, block.number, serialized_block)?;
                    chunk_id
                }
            }
            None => self.create_new_chunk(last_snapshot_id, block.number, serialized_block)?,
        };

        info!(
            "Updating snapshot. Last snapshot id: {:?}, block number: {:?}, chunk id: {:?}",
            last_snapshot_id, block.number, chunk_id
        );
        self.update_snapshot(last_snapshot_id, block.number, chunk_id)?;
        self.insert_block_snapshot_id_mapping(block.number, last_snapshot_id)?;

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

/// Gets latest cometbft block height with retry if rpc server isn't available yet
async fn get_latest_comet_block_height(comet_rpc_client: &HttpClient) -> u64 {
    loop {
        match comet_rpc_client.latest_block().await {
            Ok(res) => return res.block.header.height.value(),
            Err(_e) => {
                warn!("RPC server not ready yet, retrying in 2 seconds...");
                sleep(Duration::from_secs(2)).await;
                continue;
            }
        }
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
