//! Snapshot manager is responsible for persisting snapshot chunks to disk

use crate::{comet_bft::abci::ABCIDriverMessage, Storage};
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_data_parser::{DataParser, Error as DataParserError};
use reth_db::models::ChunkId;
use reth_evm::execute::BlockExecutorProvider;
use reth_provider::{BlockReaderIdExt, ProviderError, SnapshotReader, SnapshotWriter};
use tracing::{debug, error, info, trace, warn};

/// The default maximum size for a snapshot in bytes (8 MB).
pub const DEFAULT_MAX_SNAPSHOT_SIZE_BYTES: u64 = 8 * 1024 * 1024; // 8 Mbs max size
/// The default size for a snapshot chunk in bytes (1 MB).
pub const DEFAULT_SNAPSHOT_CHUNK_SIZE_BYTES: u64 = 1 * 1024 * 1024; // 1 MB
/// The default number of recent snapshots to keep.
pub const DEFAULT_SNAPSHOT_KEEP_RECENT: u64 = 3;

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
    max_snapshot_size_bytes: u64,
    snapshot_chunk_size_bytes: u64,
    snapshot_keep_recent: u64,
}

impl<EF, BF, DB> SnapshotManager<EF, BF, DB>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    DB: BlockReaderIdExt + Clone + 'static,
{
    pub(crate) fn new(
        storage: Storage<EF, BF, DB>,
        compressor: DataParser,
        snapshot_manager_tx: tokio::sync::mpsc::Receiver<ABCIDriverMessage>,
    ) -> Self {
        Self {
            storage,
            compressor,
            snapshot_manager_tx,
            max_snapshot_size_bytes: DEFAULT_MAX_SNAPSHOT_SIZE_BYTES,
            snapshot_chunk_size_bytes: DEFAULT_SNAPSHOT_CHUNK_SIZE_BYTES,
            snapshot_keep_recent: DEFAULT_SNAPSHOT_KEEP_RECENT,
        }
    }
}

impl<EF, BF, DB> SnapshotRunnable for SnapshotManager<EF, BF, DB>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    DB: BlockReaderIdExt + SnapshotWriter + SnapshotReader + Clone + 'static,
{
    // Note: this function should not be called unless we are fully synced
    async fn run(&mut self) -> Result<(), SnapshotManagerError> {
        trace!(target: "consensus::authority::snapshot_manager::run", "started");
        let client = self.storage.client.clone();

        while let Some(abci_driver_message) = self.snapshot_manager_tx.recv().await {
            debug!(target: "consensus::authority::snapshot_manager::run", "received abci driver message {:?}", abci_driver_message);

            match abci_driver_message {
                ABCIDriverMessage::CommitBlock((sealed_block_with_peg, _cbft_hash, tx)) => {
                    // acknowledge the block
                    tx.send(()).expect("acknowledging received block send");

                    let sealed_block = sealed_block_with_peg.block();

                    // first attempt to deserialize and decompress the sealed block
                    let serialized_compressed_sealed_block = self
                    .compressor
                    .encode(sealed_block)
                    .await
                    .map_err(|e| {
                        error!(target:"consensus::authority::snapshot_manager", "Failed to serialize and compress sealed block {:?}", e);
                        SnapshotManagerError::DataParser(e)
                    })?;

                    if serialized_compressed_sealed_block.is_empty() {
                        error!(target: "consensus::authority::snapshot_manager::run", "serialized_compressed_sealed_block is empty");
                        continue;
                    }

                    // check the block height vs. the last snapshot height
                    let mut last_snapshot_id = match client.get_last_snapshot_height()? {
                        Some((last_snapshot_id, last_snapshot_height)) => {
                            if sealed_block.number < last_snapshot_height {
                                warn!(target: "consensus::authority::snapshot_manager::run", "block number {} is less than last snapshot height {}", sealed_block.number, last_snapshot_height);
                                continue;
                            }
                            last_snapshot_id
                        }
                        None => {
                            info!(target: "consensus::authority::snapshot_manager::run", "no last snapshot height. Creating a new snapshot at height {}...", sealed_block.number);
                            // create a new snapshot
                            client.create_new_snapshot(sealed_block.number, sealed_block.hash())?
                        }
                    };

                    // now check the latest snapshot height
                    let latest_snapshot_size = client.get_snapshot_size(last_snapshot_id)?;

                    // Check if there is enough space in the latest snapshot
                    debug!(target: "consensus::authority::snapshot_manager::run", "Snapshot size: {}", latest_snapshot_size);
                    if latest_snapshot_size + serialized_compressed_sealed_block.len() >
                        self.max_snapshot_size_bytes as usize
                    {
                        error!(target: "consensus::authority::snapshot_manager::run", "Snapshot size exceeds limit of {} bytes. Current size: {}, Attempted: {}", self.max_snapshot_size_bytes, latest_snapshot_size, serialized_compressed_sealed_block.len());
                        // create a new snapshot
                        last_snapshot_id =
                            client.create_new_snapshot(sealed_block.number, sealed_block.hash())?;
                    }

                    // Split the serialized block into smaller chunks
                    let chunks = serialized_compressed_sealed_block
                        .chunks(self.snapshot_chunk_size_bytes as usize);
                    debug!(target: "consensus::authority::snapshot_manager::run", "Created chunks after split: {:?}", chunks);
                    let mut new_chunks: Vec<ChunkId> = vec![];
                    for chunk in chunks {
                        let chunk_id = client.create_new_chunk(
                            last_snapshot_id,
                            sealed_block.number,
                            chunk.to_vec(),
                        )?;
                        new_chunks.push(chunk_id);
                        client.update_snapshot(last_snapshot_id, sealed_block.number, chunk_id)?;
                    }
                    client.create_block_chunks_register(sealed_block.number, new_chunks)?;

                    // check if we need to delete older snapshots (Retention policy)
                    if client.get_snapshots_count()? > self.snapshot_keep_recent as usize {
                        client.remove_oldest_snapshot()?;
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
