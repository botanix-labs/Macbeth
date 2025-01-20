//! Snapshot manager is responsible for persisting snapshot chunks to disk

use std::sync::Arc;

use crate::{comet_bft::abci::ABCIDriverMessage, Storage};
use bytes::Bytes;
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_data_parser::{DataParser, Error as DataParserError};
use reth_db::{models::ChunkId, DatabaseEnv};
use reth_db_api::database::Database;
use reth_evm::execute::BlockExecutorProvider;
use reth_node_core::args::StateSyncArgs;
use reth_provider::{
    BlockReaderIdExt, DatabaseProviderRW, ProviderError, ProviderFactory, SnapshotReader,
    SnapshotWriter,
};
use tracing::{debug, error, info, trace, warn};

/// Snapshot manager error
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum SnapshotManagerError {
    #[error("db provider error: {0}")]
    /// Error related to the database provider
    Provider(#[from] ProviderError),
    /// Error related to the data parser
    #[error("Data parser error: {0}")]
    DataParser(#[from] DataParserError),
}

/// Snapshot manager monitoring trait
#[allow(dead_code)]
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
    snapshot_manager_tx: tokio::sync::mpsc::Receiver<ABCIDriverMessage>,
    state_sync_args: StateSyncArgs,
    state_lock: Arc<RwLock<SnapshotManagerStateLock>>,
}

impl<EF, BF, DB> SnapshotManager<EF, BF, DB>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    DB: BlockReaderIdExt + SnapshotWriter + SnapshotReader + Clone + 'static,
{
    pub(crate) fn new(
        storage: Storage<EF, BF, DB>,
        compressor: DataParser,
        snapshot_manager_tx: tokio::sync::mpsc::Receiver<ABCIDriverMessage>,
        state_sync_args: StateSyncArgs,
        state_lock: Arc<RwLock<SnapshotManagerStateLock>>,
    ) -> Self {
        Self { storage, compressor, snapshot_manager_tx, state_sync_args, state_lock }
    }

    /// Create a new snapshot
    pub fn create_new_snapshot(
        &self,
        sealed_block: &SealedBlockWithSenders,
        app_hash: Bytes,
    ) -> Result<SnapshotId, SnapshotManagerError> {
        let provider_rw = self.storage.provider_factory.provider_rw()?;
        let snapshot_id = provider_rw.create_new_snapshot(
            sealed_block.number,
            sealed_block.hash(),
            app_hash.as_ref(),
        )?;
        provider_rw.commit()?;
        Ok(snapshot_id)
    }

    /// Remove oldest snapshot
    pub fn remove_oldest_snapshot(&self) -> Result<(), SnapshotManagerError> {
        let provider_rw = self.storage.provider_factory.provider_rw()?;
        provider_rw.remove_oldest_snapshot()?;
        provider_rw.commit()?;
        Ok(())
    }

    /// Create a new chunk
    pub fn create_new_chunk(
        &self,
        snapshot_id: SnapshotId,
        block_id: BlockNumber,
        chunk_data: Vec<u8>,
    ) -> Result<ChunkId, SnapshotManagerError> {
        let provider_rw = self.storage.provider_factory.provider_rw()?;
        let chunk_id = provider_rw.create_new_chunk(snapshot_id, block_id, chunk_data)?;
        provider_rw.commit()?;
        Ok(chunk_id)
    }

    /// Create block chunks register
    pub fn create_block_chunks_register(
        &self,
        block_id: BlockNumber,
        chunk_ids: Vec<ChunkId>,
    ) -> Result<(), SnapshotManagerError> {
        let provider_rw = self.storage.provider_factory.provider_rw()?;
        provider_rw.create_block_chunks_register(block_id, chunk_ids)?;
        provider_rw.commit()?;
        Ok(())
    }

    /// Insert block snapshot id mapping
    pub fn insert_block_snapshot_id_mapping(
        &self,
        block_id: BlockNumber,
        snapshot_id: SnapshotId,
    ) -> Result<(), SnapshotManagerError> {
        let provider_rw = self.storage.provider_factory.provider_rw()?;
        provider_rw.insert_block_snapshot_id_mapping(block_id, snapshot_id)?;
        provider_rw.commit()?;
        Ok(())
    }

    /// Get snapshot
    pub fn update_snapshot(
        &self,
        snapshot_id: SnapshotId,
        block_id: BlockNumber,
        chunk_id: ChunkId,
    ) -> Result<(), SnapshotManagerError> {
        let provider_rw = self.storage.provider_factory.provider_rw()?;
        provider_rw.update_snapshot(snapshot_id, block_id, chunk_id)?;
        provider_rw.commit()?;
        Ok(())
    }

    /// Get snapshots size
    pub fn get_snapshot_size(
        &self,
        last_snapshot_id: SnapshotId,
    ) -> Result<usize, SnapshotManagerError> {
        Ok(self.storage.provider_factory.provider()?.get_snapshot_size(last_snapshot_id)?)
    }

    /// Get snapshots count
    pub fn get_snapshots_count(&self) -> Result<usize, SnapshotManagerError> {
        Ok(self.storage.provider_factory.provider()?.get_snapshots_count()?)
    }

    /// Get last snapshot height
    pub fn get_last_snapshot_height(
        &self,
    ) -> Result<Option<(SnapshotId, BlockNumber)>, SnapshotManagerError> {
        Ok(self.storage.provider_factory.provider()?.get_last_snapshot_height()?)
    }

    /// Get latest header
    pub fn latest_header(&self) -> Result<Option<SealedHeader>, SnapshotManagerError> {
        Ok(self.storage.client.latest_header()?)
    }
}

impl<EF, BF, DB> SnapshotRunnable for SnapshotManager<EF, BF, DB>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    DB: BlockReaderIdExt + SnapshotWriter + SnapshotReader + Clone + 'static,
{
    async fn run(&mut self) -> Result<(), SnapshotManagerError> {
        trace!(target: "consensus::authority::snapshot_manager::run", "started");

        // let latest_block_number =
        //     client.latest_header()?.as_ref().map(|h| h.number).unwrap_or_default();
        // if latest_block_number > 0 {
        //     warn!(target: "consensus::authority::snapshot_manager::run", "State sync will not run
        // as it requires an empty state but it currently has a block number of {}",
        // latest_block_number);     return Ok(());
        // }

        while let Some(abci_driver_message) = self.snapshot_manager_tx.recv().await {
            debug!(target: "consensus::authority::snapshot_manager::run", "received abci driver message {:?}", abci_driver_message);

            match abci_driver_message {
                ABCIDriverMessage::CommitBlock((sealed_block_with_peg, cbft_hash, _tx)) => {
                    // acknowledge the block
                    tx.send(()).expect("acknowledging received block send");

                    let sealed_block = sealed_block_with_peg.block();
                    self.storage
                        .provider_factory
                        .provider_rw()?
                        .create_new_snapshot(sealed_block.number, sealed_block.hash())?;

                    // first attempt to deserialize and decompress the sealed block
                    let serialized_compressed_sealed_block = self.compressor.encode(sealed_block).await.map_err(|e| {
                            error!(target:"consensus::authority::snapshot_manager", "Failed to serialize and compress sealed block {:?}", e);
                            SnapshotManagerError::DataParser(e)
                        })?;

                    if serialized_compressed_sealed_block.is_empty() {
                        error!(target: "consensus::authority::snapshot_manager::run", "serialized_compressed_sealed_block is empty");
                        continue;
                    }

                    // check the block height vs. the last snapshot height
                    let mut last_snapshot_id = match self.get_last_snapshot_height()? {
                        Some((last_snapshot_id, last_snapshot_height)) => {
                            if sealed_block.number < last_snapshot_height {
                                warn!(target: "consensus::authority::snapshot_manager::run", "block number {} is less than last snapshot height {}", sealed_block.number, last_snapshot_height);
                                continue;
                            }
                            last_snapshot_id
                        }
                        None => {
                            info!(target: "consensus::authority::snapshot_manager::run", "no last snapshot height. Creating a new snapshot at height {}...", sealed_block.number); // create a new snapshot
                            self.create_new_snapshot(
                                sealed_block,
                                Bytes::from(prost::bytes::Bytes::copy_from_slice(&cbft_hash.0)),
                            )?
                        }
                    };

                    info!("Last_snapshot_id: {:?}", last_snapshot_id);

                    // now check the latest snapshot size
                    let latest_snapshot_size = self.get_snapshot_size(last_snapshot_id)?;
                    info!(
                        "++++++++++++++++++++ SNAPSHOTS COUNT: {:?}",
                        self.storage.provider_factory.provider_rw()?.get_snapshots_count()?
                    );

                    // Check if there is enough space in the latest snapshot
                    debug!(target: "consensus::authority::snapshot_manager::run", "Snapshot size: {}", latest_snapshot_size);
                    if latest_snapshot_size + serialized_compressed_sealed_block.len() >
                        self.state_sync_args.max_snapshot_size_bytes
                    {
                        info!(target: "consensus::authority::snapshot_manager::run", "Snapshot size exceeds limit of {} bytes. Current size: {}, Attempted: {}", self.state_sync_args.max_snapshot_size_bytes, latest_snapshot_size, serialized_compressed_sealed_block.len());
                        // create a new snapshot
                        last_snapshot_id = self.create_new_snapshot(
                            sealed_block,
                            Bytes::from(prost::bytes::Bytes::copy_from_slice(&cbft_hash.0)),
                        )?;
                        info!("Created last_snapshot_id: {:?}", last_snapshot_id);
                    }
                    info!("Snapshots count: {:?}", self.get_snapshots_count()?);

                    // if serialized_compressed_sealed_block.is_empty() {
                    //     error!(target: "consensus::authority::snapshot_manager::run",
                    // "serialized_compressed_sealed_block is empty");
                    //     continue;
                    // }

                    // Split the serialized block into smaller chunks
                    let chunks = serialized_compressed_sealed_block
                        .chunks(self.state_sync_args.snapshot_chunk_size_bytes);
                    info!(target: "consensus::authority::snapshot_manager::run", "Created chunks after split: {:?}", chunks.len());
                    let mut new_chunk_ids: Vec<ChunkId> = vec![];
                    for chunk in chunks {
                        let chunk_id = self.create_new_chunk(
                            last_snapshot_id,
                            sealed_block.number,
                            chunk.to_vec(),
                        )?;
                        new_chunk_ids.push(chunk_id);
                        info!(
                            "Updating snapshot with: {:?} {:?} {:?}",
                            last_snapshot_id, sealed_block.number, chunk_id
                        );
                        self.update_snapshot(last_snapshot_id, sealed_block.number, chunk_id)?;
                    }
                    self.create_block_chunks_register(sealed_block.number, new_chunk_ids)?;
                    self.insert_block_snapshot_id_mapping(sealed_block.number, last_snapshot_id)?;

                    // check if we need to delete older snapshots (Retention policy)
                    if self.get_snapshots_count()? >
                        self.state_sync_args.snapshot_keep_recent as usize
                    {
                        let oldest_snapshot_height = self
                            .storage
                            .provider_factory
                            .get_first_snapshot_height()?
                            .map(|(_snapshot_id, snapshot_height)| snapshot_height)
                            .unwrap_or_default();
                        let state_lock =
                            self.state_lock.read().expect("snapshot state sync locked");
                        let locked_block_height = state_lock.block_id;
                        drop(state_lock);
                        // make sure we are not deleting the height we are just syncing with
                        if locked_block_height == oldest_snapshot_height {
                            info!(target: "consensus::authority::snapshot_manager::run", "Removing oldest snapshot at height {}", oldest_snapshot_height);
                            self.remove_oldest_snapshot()?;
                        }
                    }
                }
                ABCIDriverMessage::Exit => {
                    debug!(target: "consensus::authority::snapshot_manager::run", "exiting");
                    return Ok(());
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

    #[test]
    fn test_rw_snapshots() {
        let provider_factory = create_test_provider_factory_with_chain_spec(MAINNET.clone());

        init_genesis(provider_factory.clone()).unwrap();
        // let client = BlockchainProvider::new(
        //     provider_factory.clone(),
        //     Arc::new(NoopBlockchainTree::default()),
        // )
        // .unwrap();

        let client = provider_factory.provider_rw().unwrap();

        // insert a new snapshot at block height 1
        client.create_new_snapshot(1, B256::random(), &vec![]).unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;

        let snapshots = client.get_snapshots().unwrap();
        let snapshots_count = client.get_snapshots_count().unwrap();
        println!("snapshots: {:?} {:?}", snapshots, snapshots_count);

        let last_snapshot = client.get_last_snapshot_height().unwrap();
        println!("last snapshot height: {:?}", last_snapshot);

        // insert a new snapshot at block height 2
        client.create_new_snapshot(2, B256::random(), &vec![]).unwrap();

        let snapshots = client.get_snapshots().unwrap();
        let snapshots_count = client.get_snapshots_count().unwrap();
        println!("snapshots: {:?} {:?}", snapshots, snapshots_count);

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
        let snp = snapshots.iter().next().unwrap().clone();
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
        let snapshot_id = client.create_new_snapshot(block_id, B256::random(), &vec![]).unwrap();

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
        let snapshot_id = client.create_new_snapshot(block_id, B256::random(), &vec![]).unwrap();

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
        let snapshot_id = client.create_new_snapshot(block_id, B256::random(), &vec![]).unwrap();
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
        let chunk_data: Vec<u8> = vec![1, 2, 3, 4, 5];
        let block_ids = 1..=10;
        // loop over block_heights
        let mut chunk_id = 0;
        for block_id in block_ids.clone() {
            chunk_id = client.create_new_chunk(snapshot_id, block_id, chunk_data.clone()).unwrap();
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
        let mut id = client.create_new_snapshot_sync(1, B256::random(), 10, 1).unwrap();
        id = client.create_new_snapshot_sync(2, B256::random(), 20, 1).unwrap();
        id = client.create_new_snapshot_sync(3, B256::random(), 30, 1).unwrap();

        let mut snapshot_sync_by_id = client.get_snapshot_sync_by_id(id).unwrap().unwrap();
        assert!(snapshot_sync_by_id.height() == 3);
        assert!(snapshot_sync_by_id.total_chunks() == 30);
        assert!(snapshot_sync_by_id.format() == 1);
        assert!(snapshot_sync_by_id.last_applied_chunk_index() == 0);

        let last_snapshot_sync_id = client.get_last_snapshot_sync_id().unwrap().unwrap();
        assert!(last_snapshot_sync_id == 3);

        snapshot_sync_by_id.set_height(33);
        snapshot_sync_by_id.set_total_chunks(44);
        snapshot_sync_by_id.append_chunk_data(vec![1, 2, 3, 4, 5]);
        client.update_snapshot_sync(3, snapshot_sync_by_id).unwrap();

        let updated_snapshot_sync_by_id = client.get_snapshot_sync_by_id(id).unwrap().unwrap();
        assert!(updated_snapshot_sync_by_id.height() == 33);
        assert!(updated_snapshot_sync_by_id.total_chunks() == 44);
        assert!(updated_snapshot_sync_by_id.format() == 1);
        assert!(updated_snapshot_sync_by_id.last_applied_chunk_index() == 0);
        assert!(updated_snapshot_sync_by_id.data().to_vec() == vec![1, 2, 3, 4, 5]);
    }
}
