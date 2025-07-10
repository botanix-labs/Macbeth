//! Models for snapshots and chunks.

use reth_codecs::{add_arbitrary_tests, Compact};
use reth_primitives::{BlockNumber, Bytes, B256};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// A snapshot sync id.
///
/// Unique identifier for snapshot synchronization operations. Each sync
/// operation gets a unique ID that allows tracking progress and state
/// throughout the synchronization process.
///
/// # Value Range
///
/// Valid values are any positive 64-bit integer. Zero may be reserved
/// for special cases depending on implementation.
pub type SnapshotSyncId = u64;

/// A snapshot id.
///
/// Unique identifier for blockchain snapshots. Each snapshot represents
/// a point-in-time state of the blockchain and is assigned a unique ID
/// for referencing and management.
///
/// # Value Range
///
/// Valid values are any positive 64-bit integer. IDs are typically
/// assigned sequentially but this is not guaranteed.
pub type SnapshotId = u64;

/// A chunk id.
///
/// Unique identifier for snapshot chunks. Chunks are segments of snapshot
/// data that allow for efficient storage and transmission of large snapshots.
/// Each chunk gets a unique ID for tracking and assembly.
///
/// # Value Range
///
/// Valid values are any positive 64-bit integer. Chunk IDs are typically
/// assigned sequentially within a snapshot but may not be globally sequential.
pub type ChunkId = u64;

/// A chunk index.
///
/// Index position of a chunk within a snapshot. Unlike ChunkId which is
/// a unique identifier, this represents the sequential position of a chunk
/// within its parent snapshot (0, 1, 2, etc.).
///
/// # Value Range
///
/// Valid values start from 0 and increment sequentially. The maximum value
/// depends on the number of chunks in a snapshot.
pub type SnapshotChunkIndex = u64;

/// A snapshot hash is a keccak hash of a snapshot.
pub type SnapshotChunkHash = B256;

/// The storage of the a single chunk within a snapshot.
/// Chunks are many blocks with senders
/// It is expected for the same snapshot to have multiple chunks
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct SnapshotChunk {
    /// The snapshot id
    snapshot_id: u64,
    /// The data of the chunk
    chunk_data: Vec<Bytes>,
    /// Starting Block Number
    starting_block_number: BlockNumber,
    /// Ending Block Number
    ending_block_number: BlockNumber,
}

impl SnapshotChunk {
    /// Creates a new snapshot chunk for a given snapshot id.
    /// 
    /// This constructor initializes a new chunk with the provided data and sets
    /// both the starting and ending block numbers to the same value. Additional
    /// data can be appended later using `append_chunk_data`.
    /// 
    /// # Parameters
    /// 
    /// * `snapshot_id` - The ID of the snapshot this chunk belongs to
    /// * `starting_block_number` - The block number where this chunk starts
    /// * `chunk_data` - The initial data for this chunk
    /// 
    /// # Returns
    /// 
    /// A new `SnapshotChunk` instance ready for use.
    pub fn new(
        snapshot_id: SnapshotId,
        starting_block_number: BlockNumber,
        chunk_data: Vec<u8>,
    ) -> Self {
        Self {
            snapshot_id,
            chunk_data: vec![Bytes::from(chunk_data)],
            starting_block_number,
            ending_block_number: starting_block_number,
        }
    }

    /// Appends data to the existing chunk data.
    ///
    /// Adds additional block data to this chunk and updates the ending block number.
    /// This allows chunks to span multiple blocks efficiently while maintaining
    /// proper block range tracking.
    ///
    /// # Parameters
    ///
    /// * `additional_data` - The block data to append to this chunk
    /// * `ending_block_number` - The block number of the appended data
    ///
    /// # Behavior
    ///
    /// - The new data is added to the chunk's data collection
    /// - The ending block number is updated to reflect the new range
    /// - The starting block number remains unchanged
    pub fn append_chunk_data(
        &mut self,
        additional_data: Vec<u8>,
        ending_block_number: BlockNumber,
    ) {
        self.chunk_data.push(Bytes::from(additional_data));
        self.ending_block_number = ending_block_number;
    }

    /// Return the size of this chunk.
    ///
    /// Calculates the total memory footprint of this chunk including metadata
    /// and all stored block data. This is useful for memory management and
    /// determining when chunks should be split or persisted.
    ///
    /// # Returns
    ///
    /// The total size in bytes, including:
    /// - Chunk ID storage (8 bytes)
    /// - All block data chunks (variable size)
    ///
    /// # Performance
    ///
    /// This method iterates through all data chunks, so the time complexity
    /// is O(n) where n is the number of data chunks in this snapshot chunk.
    pub fn size(&self) -> usize {
        let chunk_id_size = std::mem::size_of::<u64>();
        let data_size = self.chunk_data.iter().map(|data| data.len()).sum::<usize>();
        chunk_id_size + data_size
    }

    /// Return the snapshot id of this chunk.
    pub const fn snapshot_id(&self) -> SnapshotId {
        self.snapshot_id
    }

    /// Return the data of this chunk.
    ///
    /// Provides read-only access to all block data stored in this chunk.
    /// Each data element represents the serialized data for one or more blocks,
    /// including transaction data and associated senders.
    ///
    /// # Returns
    ///
    /// A slice containing all data chunks in the order they were added.
    /// Each `Bytes` element contains serialized block data.
    ///
    /// # Usage
    ///
    /// ```rust,ignore
    /// for data in chunk.chunk_data() {
    ///     // Process serialized block data
    ///     let block_data = deserialize_block_data(data)?;
    /// }
    /// ```
    pub fn chunk_data(&self) -> &[Bytes] {
        self.chunk_data.as_ref()
    }

    /// Return the ending block number of this chunk.
    pub const fn get_ending_block_number(&self) -> BlockNumber {
        self.ending_block_number
    }

    /// Return the starting block number of this chunk.
    pub const fn get_starting_block_number(&self) -> BlockNumber {
        self.starting_block_number
    }
}

/// Snapshot data structure
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct Snapshot {
    /// The snapshot id
    id: u64,
    /// The snapshot height (same as the block height)
    height: u64,
    /// The snapshot chunks ids
    chunk_ids: Vec<ChunkId>,
    /// The snapshot block ids
    /// TODO: this could be start and end block number not a vec
    block_ids: Vec<BlockNumber>,
    /// The hash of the block at that height
    block_hash: B256,
}

impl Snapshot {
    /// Creates a new snapshot by given height and `block_hash`.
    /// 
    /// This constructor initializes a new snapshot with empty chunk and block
    /// ID collections. Chunks and blocks can be added later using the
    /// appropriate methods.
    /// 
    /// # Parameters
    /// 
    /// * `id` - Unique identifier for this snapshot
    /// * `height` - Block height at which this snapshot was taken
    /// * `block_hash` - Hash of the block at the snapshot height
    /// 
    /// # Returns
    /// 
    /// A new `Snapshot` instance ready for use.
    pub const fn new(id: u64, height: u64, block_hash: B256) -> Self {
        Self { id, height, chunk_ids: Vec::new(), block_ids: Vec::new(), block_hash }
    }

    /// Sets the snapshot id.
    ///
    /// Updates the unique identifier for this snapshot. This should be used
    /// with caution as changing the ID may break references from other components.
    ///
    /// # Parameters
    ///
    /// * `id` - The new unique identifier for this snapshot
    ///
    /// # Warning
    ///
    /// Changing the snapshot ID may break existing relationships with chunks,
    /// blocks, and other database entities that reference this snapshot.
    pub fn set_id(&mut self, id: u64) {
        self.id = id;
    }

    /// Sets the snapshot height.
    pub fn set_height(&mut self, height: u64) {
        self.height = height;
    }

    /// Adds a chunk id to the snapshot.
    ///
    /// Associates a new chunk with this snapshot by adding its ID to the
    /// chunk collection. This establishes the relationship between the
    /// snapshot and its component chunks.
    ///
    /// # Parameters
    ///
    /// * `chunk` - The unique identifier of the chunk to associate with this snapshot
    ///
    /// # Behavior
    ///
    /// The chunk ID is appended to the existing list. No duplicate checking
    /// is performed - use `add_chunk_id_if_not_exists()` for that functionality.
    pub fn add_chunk_id(&mut self, chunk: ChunkId) {
        self.chunk_ids.push(chunk);
    }

    /// Sets the snapshot chunks, replacing the existing ones.
    pub fn set_chunks(&mut self, chunks: Vec<ChunkId>) {
        self.chunk_ids = chunks;
    }

    /// Adds a block ID to the snapshot.
    pub fn add_block_id(&mut self, block_id: u64) {
        self.block_ids.push(block_id);
    }

    /// Sets the snapshot block IDs, replacing the existing ones.
    pub fn set_block_ids(&mut self, block_ids: Vec<u64>) {
        self.block_ids = block_ids;
    }

    /// Sets the block hash of the snapshot.
    pub fn set_block_hash(&mut self, block_hash: B256) {
        self.block_hash = block_hash;
    }

    /// Get latest chunk id
    ///
    /// Returns the ID of the most recently added chunk in this snapshot.
    /// This is useful for determining the current state of chunk creation
    /// and for appending new data to the latest chunk.
    ///
    /// # Returns
    ///
    /// * `Some(ChunkId)` - The ID of the latest chunk if any chunks exist
    /// * `None` - If this snapshot has no associated chunks
    ///
    /// # Usage
    ///
    /// This is commonly used when deciding whether to append data to an
    /// existing chunk or create a new one.
    pub fn get_latest_chunk_id(&self) -> Option<ChunkId> {
        self.chunk_ids.last().copied()
    }

    /// Get oldest chunk id
    pub fn get_oldest_chunk_id(&self) -> Option<ChunkId> {
        self.chunk_ids.first().copied()
    }

    /// Adds a block ID to the snapshot if it doesn't already exist.
    /// Returns `true` if the block ID was added, `false` if it was already present.
    pub fn add_block_id_if_not_exists(&mut self, block_id: BlockNumber) -> bool {
        let mut block_ids_set: BTreeSet<u64> = self.block_ids.iter().copied().collect();
        if block_ids_set.insert(block_id) {
            self.block_ids.push(block_id);
            true
        } else {
            false
        }
    }

    /// Adds a chunk ID to the snapshot if it doesn't already exist.
    /// Returns `true` if the block ID was added, `false` if it was already present.
    pub fn add_chunk_id_if_not_exists(&mut self, chunk_id: ChunkId) -> bool {
        let mut chunk_ids_set: BTreeSet<u64> = self.chunk_ids.iter().copied().collect();
        if chunk_ids_set.insert(chunk_id) {
            self.chunk_ids.push(chunk_id);
            true
        } else {
            false
        }
    }

    /// Calculates the total size in bytes of this snapshot
    pub fn size(&self) -> usize {
        // Size of u64 field (8 bytes)
        let height_size = std::mem::size_of::<u64>();

        // Size of B256 block hash (32 bytes)
        let hash_size = std::mem::size_of::<B256>();

        // Size of all block ids (each u64 is 8 bytes)
        let block_ids_size = self.block_ids.len() * std::mem::size_of::<u64>();

        // Size of all chunk ids (each u64 is 8 bytes)
        let chunk_ids_size = self.chunk_ids.len() * std::mem::size_of::<u64>();

        height_size + hash_size + block_ids_size + chunk_ids_size
    }

    /// Return the snapshot id.
    ///
    /// Gets the unique identifier for this snapshot. This ID is used
    /// throughout the system to reference and manage the snapshot.
    ///
    /// # Returns
    ///
    /// The unique 64-bit identifier for this snapshot.
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// Return the snapshot height.
    pub const fn height(&self) -> u64 {
        self.height
    }

    /// Return the chunk ids.
    pub fn chunk_ids(&self) -> &[ChunkId] {
        self.chunk_ids.as_ref()
    }

    /// Return the block ids.
    pub fn block_ids(&self) -> &[u64] {
        self.block_ids.as_ref()
    }

    /// Return the hash of this snapshot block.
    pub const fn block_hash(&self) -> B256 {
        self.block_hash
    }

    /// Gets the snapshot hash.
    /// 
    /// This method computes a deterministic hash of the snapshot by combining
    /// all its components: ID, height, chunk IDs, block IDs, and block hash.
    /// The hash is computed using SHA-256 and can be used for verification
    /// and comparison of snapshots across network nodes.
    /// 
    /// # Returns
    /// 
    /// A 32-byte SHA-256 hash as a `Vec<u8>`.
    pub fn get_hash(&self) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(self.id.to_le_bytes());
        hasher.update(self.height.to_le_bytes());
        for chunk_id in &self.chunk_ids {
            hasher.update(chunk_id.to_le_bytes());
        }
        for block_id in &self.block_ids {
            hasher.update(block_id.to_le_bytes());
        }
        hasher.update(self.block_hash);
        hasher.finalize().to_vec()
    }
}

/// SnapshotSync data structure
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct SnapshotSync {
    /// The snapshot height (same as the block height
    height: u64,
    /// Total chunks
    total_chunks: u64,
    /// The last applied chunk index
    last_applied_chunk_index: u64,
    /// The snapshot hash
    snapshot_hash: B256,
    /// The application-specific snapshot format
    format: u64,
}

impl SnapshotSync {
    /// Creates a new snapshot sync by given height and `block_hash`
    pub const fn new(height: u64, snapshot_hash: B256, format: u64, total_chunks: u64) -> Self {
        Self { height, total_chunks, last_applied_chunk_index: 0, snapshot_hash, format }
    }

    /// Sets the snapshot height.
    pub fn set_height(&mut self, height: u64) {
        self.height = height;
    }

    /// Sets the total chunks.
    pub fn set_total_chunks(&mut self, total_chunks: u64) {
        self.total_chunks = total_chunks;
    }

    /// Sets the `last_applied_chunk_index`.
    pub fn set_last_applied_chunk_index(&mut self, last_applied_chunk_index: u64) {
        self.last_applied_chunk_index = last_applied_chunk_index;
    }

    /// Return the height.
    pub const fn height(&self) -> u64 {
        self.height
    }

    /// Return the hash of this snapshot block.
    pub const fn snapshot_hash(&self) -> B256 {
        self.snapshot_hash
    }

    /// Return the number of total chunks.
    pub const fn total_chunks(&self) -> u64 {
        self.total_chunks
    }

    /// Return the `last_applied_chunk_index`.
    pub const fn last_applied_chunk_index(&self) -> u64 {
        self.last_applied_chunk_index
    }

    /// Return the format.
    pub const fn format(&self) -> u64 {
        self.format
    }
}

#[cfg(test)]
mod tests {
    use reth_primitives::hex;

    use super::*;

    #[test]
    fn snapshot_chunks_test() {
        let _chunks = vec![
            SnapshotChunk {
                snapshot_id: 1,
                chunk_data: Vec::new(),
                starting_block_number: 1001,
                ending_block_number: 1001,
            },
            SnapshotChunk {
                snapshot_id: 1,
                chunk_data: Vec::new(),
                starting_block_number: 1002,
                ending_block_number: 1002,
            },
        ];
        let block_hash = B256::random();
        let snapshot = Snapshot {
            id: 100,
            height: 12000,
            block_ids: vec![1001],
            chunk_ids: vec![1, 2],
            block_hash: block_hash.clone(),
        };

        assert_eq!(snapshot.id(), 100);
        assert_eq!(snapshot.chunk_ids(), &vec![1, 2]);
        assert_eq!(snapshot.block_hash(), block_hash);
        assert_eq!(snapshot.block_ids(), vec![1001]);
        assert_eq!(snapshot.height(), 12000);
    }

    #[test]
    // We don't care about deserialize and serialize here
    // As long as the hash function is deterministic,
    // Comet can use the hash to ensure snapshots are the same across nodes
    fn set_hash_should_hash_the_snapshot() {
        let snapshot = Snapshot {
            id: 100,
            height: 12000,
            block_ids: vec![1001],
            chunk_ids: vec![1, 2],
            block_hash: B256::ZERO,
        };
        let snapshot_hash = snapshot.get_hash();

        assert_eq!(
            hex::encode(snapshot_hash),
            "55418ead0d08a6acc2544763f47641046787942f196eaf4a3b7de4f7c6d94e98"
        );
    }
}
