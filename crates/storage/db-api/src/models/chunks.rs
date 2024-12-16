use reth_codecs::{add_arbitrary_tests, Compact};
use reth_primitives::{TxNumber, B256};
use serde::{Deserialize, Serialize};

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
pub struct SnapshotChunk {
    /// The snapshot height (same as the block height)
    pub snapshot_height: u64,
    /// The chunk index within the snapshot
    pub chunk_index: u64,
    /// The hash of the chunk
    pub chunk_hash: B256,
    /// The data of the chunk
    pub chunk_data: Vec<u8>,
}

impl SnapshotChunk {
    /// Return the snapshot height of this chunk.
    pub const fn snapshot_height(&self) -> u64 {
        self.snapshot_height
    }

    /// Return the index of this chunk.
    pub const fn chunk_index(&self) -> u64 {
        self.chunk_index
    }

    /// Return the hash of this chunk.
    pub const fn chunk_hash(&self) -> B256 {
        self.chunk_hash
    }

    /// Return the data of this chunk.
    pub fn chunk_data(&self) -> &[u8] {
        self.chunk_data.as_slice()
    }
}

#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct Snapshot {
    /// The snapshot height (same as the block height or the snapshot id)
    pub height: u64,
    /// The snapshot chunks
    pub chunks: Vec<SnapshotChunk>,
    /// The snapshot block ids
    pub block_ids: Vec<u64>,
    /// The hash of the block at that height
    pub block_hash: B256,
}

impl Snapshot {
    /// Return the snapshot height.
    pub const fn height(&self) -> u64 {
        self.height
    }

    /// Return the chunks.
    pub fn chunks(&self) -> &[SnapshotChunk] {
        &self.chunks
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

// #[cfg(test)]
// mod tests {
//     use crate::StoredBlockBodyIndices;

//     #[test]
//     fn block_indices() {
//         let first_tx_num = 10;
//         let tx_count = 6;
//         let block_indices = StoredBlockBodyIndices { first_tx_num, tx_count };

//         assert_eq!(block_indices.first_tx_num(), first_tx_num);
//         assert_eq!(block_indices.last_tx_num(), first_tx_num + tx_count - 1);
//         assert_eq!(block_indices.next_tx_num(), first_tx_num + tx_count);
//         assert_eq!(block_indices.tx_count(), tx_count);
//         assert_eq!(block_indices.tx_num_range(), first_tx_num..first_tx_num + tx_count);
//     }
// }
