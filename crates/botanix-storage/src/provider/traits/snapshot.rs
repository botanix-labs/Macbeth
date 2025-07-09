use crate::models::{ChunkId, Snapshot, SnapshotChunk, SnapshotId, SnapshotSync, SnapshotSyncId};
use reth_primitives::{BlockNumber, B256};
use reth_storage_errors::provider::ProviderResult;
use std::ops::RangeInclusive;

/// SnapshotReader
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait SnapshotReader: Send + Sync {
    /// Get snapshots
    fn get_snapshots(&self) -> ProviderResult<Vec<Snapshot>>;

    /// Get snapshot by id
    fn get_snapshot_by_id(&self, snapshot_id: SnapshotId) -> ProviderResult<Option<Snapshot>>;

    /// Get last snapshot sync by id
    fn get_last_snapshot_sync_id(&self) -> ProviderResult<Option<SnapshotSyncId>>;

    /// Get snapshot sync by height
    fn get_snapshot_sync_by_height(&self, height: u64) -> ProviderResult<Option<SnapshotSync>>;

    /// Get snapshot sync by id
    fn get_snapshot_sync_by_id(&self, id: u64) -> ProviderResult<Option<SnapshotSync>>;

    /// Get chunk by chunk id
    fn get_chunk_by_id(&self, chunk_id: ChunkId) -> ProviderResult<Option<SnapshotChunk>>;

    /// Get chunk size
    fn get_chunk_size(&self, chunk_id: ChunkId) -> ProviderResult<usize>;

    /// Get snapshot id by block id
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

    /// Get snapshot size
    fn get_snapshots_count(&self) -> ProviderResult<usize>;

    /// Get latest chunk id
    fn get_last_chunk_id(&self) -> ProviderResult<Option<ChunkId>>;

    /// Get first chunk id
    fn get_first_chunk_id(&self) -> ProviderResult<Option<ChunkId>>;
}

/// SnapshotWriter
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
    ) -> ProviderResult<SnapshotId>;

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
