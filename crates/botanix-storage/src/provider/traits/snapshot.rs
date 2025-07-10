use crate::models::{ChunkId, Snapshot, SnapshotChunk, SnapshotId, SnapshotSync, SnapshotSyncId};
use reth_primitives::{BlockNumber, B256};
use reth_storage_errors::provider::ProviderResult;
use std::ops::RangeInclusive;

/// Trait for reading snapshot data from the database.
///
/// This trait provides read-only access to blockchain snapshots, chunks, and
/// synchronization state. Snapshots represent point-in-time states of the
/// blockchain that can be used for fast synchronization and historical queries.
///
/// ## Snapshot Architecture
///
/// - **Snapshots**: Metadata about blockchain state at specific heights
/// - **Chunks**: Segmented data within snapshots for efficient storage/transmission
/// - **Sync State**: Progress tracking for snapshot synchronization operations
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait SnapshotReader: Send + Sync {
    /// Get snapshots
    ///
    /// Retrieves all snapshots stored in the database. This method returns
    /// a vector containing all snapshot metadata including their IDs, heights,
    /// chunk references, and block associations.
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<Snapshot>)` - A vector of all snapshots in the database
    /// * `Err(ProviderError)` - If there was an error accessing the database
    ///
    /// # Performance
    ///
    /// This method loads all snapshots into memory, so it may be expensive
    /// for databases with many snapshots. Consider using pagination or
    /// filtering methods for large datasets.
    fn get_snapshots(&self) -> ProviderResult<Vec<Snapshot>>;

    /// Get snapshot by id
    ///
    /// Retrieves a specific snapshot by its unique identifier. This is the most
    /// efficient way to access a single snapshot when you know its ID.
    ///
    /// # Parameters
    ///
    /// * `snapshot_id` - The unique identifier of the snapshot to retrieve
    ///
    /// # Returns
    ///
    /// * `Ok(Some(Snapshot))` - The snapshot if found
    /// * `Ok(None)` - If no snapshot exists with the given ID
    /// * `Err(ProviderError)` - If there was a database error
    fn get_snapshot_by_id(&self, snapshot_id: SnapshotId) -> ProviderResult<Option<Snapshot>>;

    /// Get last snapshot sync by id
    ///
    /// Retrieves the ID of the most recent snapshot synchronization operation.
    /// This is useful for tracking synchronization progress and determining
    /// the next sync operation to perform.
    ///
    /// # Returns
    ///
    /// * `Ok(Some(SnapshotSyncId))` - The ID of the last sync operation
    /// * `Ok(None)` - If no sync operations have been recorded
    /// * `Err(ProviderError)` - If there was a database error
    fn get_last_snapshot_sync_id(&self) -> ProviderResult<Option<SnapshotSyncId>>;

    /// Get snapshot sync by height
    ///
    /// Retrieves a snapshot synchronization record for a specific block height.
    /// This allows tracking synchronization progress at particular blockchain heights.
    ///
    /// # Parameters
    ///
    /// * `height` - The block height to query for synchronization data
    ///
    /// # Returns
    ///
    /// * `Ok(Some(SnapshotSync))` - The sync record if found at the given height
    /// * `Ok(None)` - If no sync record exists for the given height
    /// * `Err(ProviderError)` - If there was a database error
    fn get_snapshot_sync_by_height(&self, height: u64) -> ProviderResult<Option<SnapshotSync>>;

    /// Get snapshot sync by id
    ///
    /// Retrieves a specific snapshot synchronization record by its unique identifier.
    /// This provides access to detailed synchronization progress information.
    ///
    /// # Parameters
    ///
    /// * `id` - The unique identifier of the synchronization record
    ///
    /// # Returns
    ///
    /// * `Ok(Some(SnapshotSync))` - The sync record if found
    /// * `Ok(None)` - If no sync record exists with the given ID
    /// * `Err(ProviderError)` - If there was a database error
    fn get_snapshot_sync_by_id(&self, id: u64) -> ProviderResult<Option<SnapshotSync>>;

    /// Get chunk by chunk id
    ///
    /// Retrieves a specific snapshot chunk by its unique identifier. Chunks contain
    /// the actual snapshot data and can be quite large, so this method should be
    /// used efficiently.
    ///
    /// # Parameters
    ///
    /// * `chunk_id` - The unique identifier of the chunk to retrieve
    ///
    /// # Returns
    ///
    /// * `Ok(Some(SnapshotChunk))` - The chunk if found
    /// * `Ok(None)` - If no chunk exists with the given ID
    /// * `Err(ProviderError)` - If there was a database error
    fn get_chunk_by_id(&self, chunk_id: ChunkId) -> ProviderResult<Option<SnapshotChunk>>;

    /// Get chunk size
    ///
    /// Returns the size in bytes of a specific snapshot chunk. This is useful
    /// for memory management and progress tracking during chunk operations.
    ///
    /// # Parameters
    ///
    /// * `chunk_id` - The unique identifier of the chunk to measure
    ///
    /// # Returns
    ///
    /// * `Ok(usize)` - The size of the chunk in bytes
    /// * `Err(ProviderError)` - If there was a database error or chunk doesn't exist
    fn get_chunk_size(&self, chunk_id: ChunkId) -> ProviderResult<usize>;

    /// Get snapshot id by block id
    ///
    /// Finds the snapshot that contains or is associated with a specific block number.
    /// This is useful for determining which snapshot to use when querying historical
    /// blockchain state at a particular height.
    ///
    /// # Parameters
    ///
    /// * `block_id` - The block number to look up
    ///
    /// # Returns
    ///
    /// * `Ok(Some(SnapshotId))` - The snapshot ID if a snapshot contains this block
    /// * `Ok(None)` - If no snapshot is associated with this block number
    /// * `Err(ProviderError)` - If there was a database error
    fn get_snapshot_id_by_block_id(
        &self,
        block_id: BlockNumber,
    ) -> ProviderResult<Option<SnapshotId>>;

    /// Get block number of a chunk
    fn get_chunk_block_number(&self, chunk_id: ChunkId) -> ProviderResult<Option<BlockNumber>>;

    /// Get last snapshot height
    fn get_last_snapshot_height(&self) -> ProviderResult<Option<(SnapshotId, BlockNumber)>>;

    /// Get first snapshot height
    fn get_first_snapshot_height(&self) -> ProviderResult<Option<(SnapshotId, BlockNumber)>>;

    /// Get snapshot size
    fn get_snapshot_size(&self, snapshot_id: SnapshotId) -> ProviderResult<usize>;

    /// Get snapshots count
    ///
    /// Returns the total number of snapshots stored in the database.
    /// This is useful for monitoring storage usage and determining
    /// when snapshot cleanup may be needed.
    ///
    /// # Returns
    ///
    /// * `Ok(usize)` - The total number of snapshots
    /// * `Err(ProviderError)` - If there was a database error
    fn get_snapshots_count(&self) -> ProviderResult<usize>;

    /// Get latest chunk id
    ///
    /// Returns the ID of the most recently created chunk. This is useful
    /// for determining the current state of chunk creation and for
    /// sequential chunk processing.
    ///
    /// # Returns
    ///
    /// * `Ok(Some(ChunkId))` - The ID of the latest chunk
    /// * `Ok(None)` - If no chunks exist in the database
    /// * `Err(ProviderError)` - If there was a database error
    fn get_last_chunk_id(&self) -> ProviderResult<Option<ChunkId>>;

    /// Get first chunk id
    ///
    /// Returns the ID of the earliest created chunk. This is useful
    /// for determining the starting point for chunk processing and
    /// for cleanup operations that need to process chunks in order.
    ///
    /// # Returns
    ///
    /// * `Ok(Some(ChunkId))` - The ID of the first chunk
    /// * `Ok(None)` - If no chunks exist in the database
    /// * `Err(ProviderError)` - If there was a database error
    fn get_first_chunk_id(&self) -> ProviderResult<Option<ChunkId>>;
}

/// Trait for writing snapshot data to the database.
///
/// This trait provides write access to blockchain snapshots, chunks, and
/// synchronization state. It supports creating new snapshots, managing chunks,
/// and tracking synchronization progress.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait SnapshotWriter: Send + Sync {
    /// Create new snapshot sync
    fn create_new_snapshot_sync(
        &self,
        block_id: BlockNumber,
        snapshot_hash: B256,
        total_chunks: u64,
        format: u64,
    ) -> ProviderResult<SnapshotId>;

    /// Create new snapshot
    fn create_new_snapshot(
        &self,
        block_id: BlockNumber,
        block_hash: B256,
    ) -> ProviderResult<SnapshotId>;

    /// Create new chunk
    fn create_new_chunk(
        &self,
        snapshot_id: SnapshotId,
        block_id: BlockNumber,
        chunk_data: Vec<u8>,
    ) -> ProviderResult<ChunkId>;

    /// Append to chunk
    fn append_to_chunk(
        &self,
        chunk_id: ChunkId,
        block_number: BlockNumber,
        data: Vec<u8>,
    ) -> ProviderResult<()>;

    /// Updates a snapshot with block and chunk id
    fn update_snapshot(
        &self,
        snapshot_id: SnapshotId,
        block_id: BlockNumber,
        chunk_id: ChunkId,
    ) -> ProviderResult<()>;

    /// Updates a snapshot sync
    fn update_snapshot_sync(
        &self,
        snapshot_sync_id: SnapshotSyncId,
        updated_snapshot: SnapshotSync,
    ) -> ProviderResult<()>;

    /// Removes block snapshot id mapping
    fn remove_block_snapshot_id_mapping(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()>;

    /// Inserts block to snapshot id mapping
    fn insert_block_snapshot_id_mapping(
        &self,
        block_id: BlockNumber,
        snapshot_id: SnapshotId,
    ) -> ProviderResult<()>;

    /// Removes snapshots
    fn remove_snapshots(&self, range: RangeInclusive<SnapshotId>) -> ProviderResult<()>;

    /// Removes oldest snapshot
    fn remove_oldest_snapshot(&self) -> ProviderResult<()>;

    /// Removes snapshots
    fn remove_chunks(&self, range: RangeInclusive<ChunkId>) -> ProviderResult<()>;

    /// Deletes chunks in blocks
    fn delete_chunks_in_blocks(&self, range: RangeInclusive<ChunkId>) -> ProviderResult<()>;
}
