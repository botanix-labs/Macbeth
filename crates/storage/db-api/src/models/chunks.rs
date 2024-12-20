//! Models for snapshots and chunks.

use std::collections::HashSet;

use reth_codecs::{add_arbitrary_tests, Compact};
use reth_primitives::{BlockNumber, B256};
use serde::{Deserialize, Serialize};

/// A snapshot sync id.
pub type SnapshotSyncId = u64;

/// A snapshot id.
pub type SnapshotId = u64;

/// A chunk id.
pub type ChunkId = u64;

/// A chunk index.
pub type SnapshotChunkIndex = u64;

/// A snapshot hash is a keccak hash of a snapshot.
pub type SnapshotChunkHash = B256;

/// The storage of the a single chunk within a snapshot.
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct BlockChunksRegister {
    /// The block chunk ids
    chunk_ids: Vec<u64>,
}

impl BlockChunksRegister {
    /// Creates a new BlockChunksRegister
    pub fn new(chunk_ids: Vec<u64>) -> Self {
        Self { chunk_ids }
    }
}

/// The storage of the a single chunk within a snapshot.
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct SnapshotChunk {
    /// The snapshot id
    snapshot_id: u64,
    /// The data of the chunk
    chunk_data: Vec<u8>,
}

impl SnapshotChunk {
    /// Creates a new snapshot chunk for a given snapshot id
    pub fn new(snapshot_id: SnapshotId) -> Self {
        Self { snapshot_id, chunk_data: Vec::new() }
    }

    /// Sets the data of the chunk, replacing the existing data.
    pub fn set_chunk_data(&mut self, chunk_data: Vec<u8>) {
        self.chunk_data = chunk_data;
    }

    /// Appends data to the existing chunk data.
    pub fn append_chunk_data(&mut self, additional_data: &[u8]) {
        self.chunk_data.extend_from_slice(additional_data);
    }

    /// Return the size of this chunk.
    pub fn size(&self) -> usize {
        let snapshot_id_size = std::mem::size_of::<u64>();
        let data_size = self.chunk_data.len();
        snapshot_id_size + data_size
    }

    /// Return the snapshot id of this chunk.
    pub const fn snapshot_id(&self) -> SnapshotId {
        self.snapshot_id
    }

    /// Return the data of this chunk.
    pub fn chunk_data(&self) -> &[u8] {
        self.chunk_data.as_slice()
    }
}

/// Snapshot data structure
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct Snapshot {
    /// The snapshot height (same as the block height)
    height: u64,
    /// The snapshot chunks ids
    chunk_ids: Vec<ChunkId>,
    /// The snapshot block ids
    block_ids: Vec<BlockNumber>,
    /// The hash of the block at that height
    block_hash: B256,
}

impl Snapshot {
    /// Creates a new snapshot by given height and block_hash
    pub fn new(height: u64, block_hash: B256) -> Self {
        Self { height, chunk_ids: Vec::new(), block_ids: Vec::new(), block_hash }
    }

    /// Sets the snapshot height.
    pub fn set_height(&mut self, height: u64) {
        self.height = height;
    }

    /// Adds a chunk id to the snapshot.
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

    /// Adds a block ID to the snapshot if it doesn't already exist.
    /// Returns `true` if the block ID was added, `false` if it was already present.
    pub fn add_block_id_if_not_exists(&mut self, block_id: BlockNumber) -> bool {
        let mut block_ids_set: HashSet<u64> = self.block_ids.iter().copied().collect();
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
        let mut chunk_ids_set: HashSet<u64> = self.chunk_ids.iter().copied().collect();
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

    /// Return the snapshot height.
    pub const fn height(&self) -> u64 {
        self.height
    }

    /// Return the chunk ids.
    pub fn chunk_ids(&self) -> &[ChunkId] {
        &self.chunk_ids
    }

    /// Return the block ids.
    pub fn block_ids(&self) -> &[u64] {
        &self.block_ids
    }

    /// Return the hash of this snapshot block.
    pub const fn block_hash(&self) -> B256 {
        self.block_hash
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
    /// The combined snapshot data
    data: Vec<u8>,
    /// The application-specific snapshot format
    format: u64,
}

impl SnapshotSync {
    /// Creates a new snapshot sync by given height and block_hash
    pub fn new(height: u64, snapshot_hash: B256, format: u64, total_chunks: u64) -> Self {
        Self {
            height,
            total_chunks,
            last_applied_chunk_index: 0,
            snapshot_hash,
            data: Vec::new(),
            format,
        }
    }

    /// Sets the snapshot height.
    pub fn set_height(&mut self, height: u64) {
        self.height = height;
    }

    /// Sets the total chunks.
    pub fn set_total_chunks(&mut self, total_chunks: u64) {
        self.total_chunks = total_chunks;
    }

    /// Sets the last_applied_chunk_index.
    pub fn set_last_applied_chunk_index(&mut self, last_applied_chunk_index: u64) {
        self.last_applied_chunk_index = last_applied_chunk_index;
    }

    /// appends chunk data.
    pub fn append_chunk_data(&mut self, data: Vec<u8>) {
        self.data.extend(data);
    }

    /// appends chunk data.
    pub fn is_assembled(&self) -> bool {
        self.last_applied_chunk_index == self.total_chunks - 1
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

    /// Return the last_applied_chunk_index.
    pub const fn last_applied_chunk_index(&self) -> u64 {
        self.last_applied_chunk_index
    }

    /// Return the format.
    pub const fn format(&self) -> u64 {
        self.format
    }

    /// Return the data of this snapshot sync.
    pub fn data(&self) -> &[u8] {
        self.data.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_chunks_test() {
        let _chunks = vec![
            SnapshotChunk { snapshot_id: 1, chunk_data: Vec::new() },
            SnapshotChunk { snapshot_id: 1, chunk_data: Vec::new() },
        ];
        let snapshot = Snapshot {
            height: 12000,
            block_ids: vec![1001],
            chunk_ids: vec![1, 2],
            block_hash: Default::default(),
        };

        assert_eq!(snapshot.chunk_ids(), &vec![1, 2]);
        assert_eq!(snapshot.block_hash(), B256::default());
        assert_eq!(snapshot.block_ids(), vec![1001]);
        assert_eq!(snapshot.height(), 12000);
    }
}
