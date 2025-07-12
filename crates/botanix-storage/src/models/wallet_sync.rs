//! Models for a wallet state sync record.

use reth_codecs::{add_arbitrary_tests, Compact};
use reth_primitives::{Bytes, B256, B512};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// A peer id is a hexified peer if of a wallet state sync record.
pub type PeerID = B512;

/// A peer id is the hexified uuid for a wallet state sync record.
pub type UuidID = B256;

/// Wallet state sync record
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct WalletStateSyncRecord {
    /// the uuid of the session
    uuid: B256,
    /// The finalized pegouts data
    data: Vec<Bytes>,
    /// The blocks of each wallet state sync record
    blocks: Vec<u64>,
    /// The total number of chunks expected
    chunks_count: u64,
    /// peer id
    peer_id: B512,
}

impl WalletStateSyncRecord {
    /// Creates a new wallet state sync record.
    ///
    /// This constructor initializes a new synchronization record for a peer,
    /// optionally with initial data. The record tracks wallet state chunks
    /// and their associated block numbers for coordinated synchronization.
    ///
    /// # Parameters
    ///
    /// * `peer_id` - Unique identifier for the peer
    /// * `uuid` - Session UUID for this sync operation
    /// * `chunks_count` - Expected total number of chunks
    /// * `data` - Optional initial data as (block_number, data) tuples
    ///
    /// # Returns
    ///
    /// A new `WalletStateSyncRecord` instance ready for use.
    pub fn new(
        peer_id: PeerID,
        uuid: UuidID,
        chunks_count: u64,
        data: Option<Vec<(u64, Bytes)>>,
    ) -> Self {
        if let Some(tuples) = data {
            let (blocks, data_bytes): (Vec<u64>, Vec<Bytes>) = tuples.into_iter().unzip();
            Self { uuid, data: data_bytes, blocks, chunks_count, peer_id }
        } else {
            Self { uuid, data: Vec::new(), blocks: Vec::new(), chunks_count, peer_id }
        }
    }

    /// Appends data with block number to the existing wallet state sync record.
    ///
    /// This method adds a single data chunk along with its associated block number
    /// to the synchronization record. It ensures data uniqueness by only adding
    /// the data if it doesn't already exist in the record.
    ///
    /// # Parameters
    ///
    /// * `additional_data` - The data chunk to append to the record
    /// * `block_number` - The block number associated with this data chunk
    #[inline(always)]
    pub fn append_data_with_block(&mut self, additional_data: Bytes, block_number: u64) {
        self.add_data_if_not_exists(additional_data, block_number);
    }

    /// Appends additional data chunks with block numbers to the existing wallet state sync record.
    ///
    /// This method adds multiple data chunks along with their associated block numbers
    /// to the synchronization record in a single operation. It processes pairs of
    /// (block_number, data) and ensures uniqueness for each pair.
    ///
    /// # Parameters
    ///
    /// * `additional_data_chunks` - Vector of data chunks to append
    /// * `blocks` - Vector of block numbers corresponding to each data chunk
    ///
    /// # Behavior
    ///
    /// - Processes data chunks and block numbers in pairs using zip iteration
    /// - Each (block, data) pair is added only if neither the data nor block already exists
    /// - If the vectors have different lengths, only pairs up to the shorter length are processed
    /// - Individual pairs that already exist are skipped without affecting other pairs
    ///
    /// # Panics
    ///
    /// This method will not panic, but if `additional_data_chunks` and `blocks` have
    /// different lengths, some data may be ignored.
    pub fn append_data_block_chunks(
        &mut self,
        additional_data_chunks: Vec<Bytes>,
        blocks: Vec<u64>,
    ) {
        for (block, data) in blocks.iter().zip(additional_data_chunks) {
            self.add_data_if_not_exists(data, *block);
        }
    }

    /// Returns an iterator over block numbers with data.
    ///
    /// This method provides an iterator that yields pairs of (block_number, data)
    /// for all stored data chunks in the synchronization record. The iterator
    /// allows for efficient processing of all block-data pairs without copying.
    ///
    /// # Returns
    ///
    /// An iterator yielding tuples of `(&u64, &Bytes)` where:
    /// - First element is a reference to the block number
    /// - Second element is a reference to the corresponding data chunk
    ///
    /// # Usage
    ///
    /// ```rust,ignore
    /// for (block_number, data) in sync_record.get_blocks_data_iter() {
    ///     println!("Block {}: {} bytes", block_number, data.len());
    /// }
    /// ```
    pub fn get_blocks_data_iter(&self) -> impl Iterator<Item = (&u64, &Bytes)> {
        self.blocks.iter().zip(&self.data)
    }

    /// Return the size of this wallet state sync record.
    ///
    /// Calculates the total memory footprint of this synchronization record
    /// by summing the sizes of all its components. This is useful for memory
    /// management and for understanding storage requirements.
    ///
    /// # Returns
    ///
    /// The total size in bytes, including:
    /// - UUID (32 bytes)
    /// - Peer ID (32 bytes)
    /// - All data chunks (variable size)
    /// - All block numbers (8 bytes each)
    pub fn size(&self) -> usize {
        let uuid_size = std::mem::size_of::<B256>();
        let peer_id = std::mem::size_of::<B512>();
        let data_size = self.data.iter().map(|data| data.len()).sum::<usize>();
        let blocks_size = self.blocks.len() * std::mem::size_of::<u64>();
        uuid_size + peer_id + data_size + blocks_size
    }

    /// Return the uuid of this wallet state sync record.
    ///
    /// The UUID uniquely identifies this synchronization session and is used
    /// to coordinate wallet state synchronization between peers. Each sync
    /// session has a unique UUID that remains constant throughout the session.
    ///
    /// # Returns
    ///
    /// A 32-byte UUID that identifies this synchronization session.
    pub const fn get_uuid(&self) -> B256 {
        self.uuid
    }

    /// Return the data of this wallet state sync record.
    ///
    /// Provides read-only access to all data chunks stored in this synchronization
    /// record. The data represents wallet state information that has been synchronized
    /// with network peers.
    ///
    /// # Returns
    ///
    /// A slice containing all data chunks in the order they were added to the record.
    pub fn get_data(&self) -> &[Bytes] {
        self.data.as_ref()
    }

    /// Return the blocks of the wallet state sync records.
    ///
    /// Provides read-only access to all block numbers associated with the stored
    /// data chunks. Each block number corresponds to a data chunk at the same index.
    ///
    /// # Returns
    ///
    /// A slice containing all block numbers in the order they were added to the record.
    pub fn get_blocks(&self) -> &[u64] {
        self.blocks.as_ref()
    }

    /// Return the peer ID of this wallet state sync record.
    ///
    /// The peer ID uniquely identifies the network peer that is participating
    /// in this wallet state synchronization session. It is used to track which
    /// peer contributed which data during the synchronization process.
    ///
    /// # Returns
    ///
    /// A 64-byte identifier that uniquely identifies the peer.
    pub const fn get_peer_id(&self) -> B512 {
        self.peer_id
    }

    /// Return the chunks count of this wallet state sync record.
    ///
    /// Returns the expected total number of chunks for this synchronization session.
    /// This is used to track progress and determine when synchronization is complete.
    ///
    /// # Returns
    ///
    /// The total number of chunks expected for this synchronization session.
    pub const fn get_chunks_count(&self) -> u64 {
        self.chunks_count
    }

    /// Sets the peer id of the wallet state sync record.
    ///
    /// Updates the peer identifier for this synchronization record. This is typically
    /// used when transferring or reassigning synchronization responsibilities between peers.
    ///
    /// # Parameters
    ///
    /// * `peer_id` - The new 64-byte peer identifier to assign to this record
    pub fn set_peer_id(&mut self, peer_id: B512) {
        self.peer_id = peer_id;
    }

    /// Sets the chunks count for the wallet state sync record.
    ///
    /// Updates the expected total number of chunks for this synchronization session.
    /// This is useful when the total chunk count is determined dynamically or needs
    /// to be adjusted during synchronization.
    ///
    /// # Parameters
    ///
    /// * `chunks_count` - The new total number of chunks expected for this session
    pub fn set_chunks_count(&mut self, chunks_count: u64) {
        self.chunks_count = chunks_count;
    }

    /// Sets the uuid of the wallet state sync record.
    ///
    /// Updates the session UUID for this synchronization record. This should be used
    /// with caution as it changes the identity of the synchronization session.
    ///
    /// # Parameters
    ///
    /// * `uuid` - The new 32-byte UUID to assign to this synchronization session
    pub fn set_uuid(&mut self, uuid: B256) {
        self.uuid = uuid;
    }

    /// Adds a data chunk with its block number to the wallet state sync record if it doesn't
    /// already exist. Returns `true` if the data or block was added, `false` if it was already
    /// present.
    ///
    /// This method ensures data uniqueness by checking both the data content and block number
    /// before adding them to the record. It prevents duplicate data from being stored and
    /// maintains the integrity of the synchronization record.
    ///
    /// # Parameters
    ///
    /// * `data_chunk` - The data chunk to add to the record
    /// * `block_number` - The block number associated with the data chunk
    ///
    /// # Returns
    ///
    /// * `true` if the data chunk and block number were successfully added
    /// * `false` if either the data chunk or block number already exists in the record
    pub fn add_data_if_not_exists(&mut self, data_chunk: Bytes, block_number: u64) -> bool {
        if self.data.iter().any(|data| data == &data_chunk) {
            return false;
        }
        if self.blocks.iter().any(|block| block == &block_number) {
            return false;
        }
        self.data.push(data_chunk);
        self.blocks.push(block_number);
        true
    }

    /// Converts the blocks and data to a set of unique (block, data) tuples.
    ///
    /// This method creates a HashSet containing all (block_number, data_chunk) pairs
    /// from the synchronization record. The resulting set automatically eliminates
    /// any duplicate pairs and provides efficient lookup and set operations.
    ///
    /// # Returns
    ///
    /// A `HashSet<(u64, Bytes)>` containing unique tuples where:
    /// - First element is the block number
    /// - Second element is the corresponding data chunk
    pub fn blocks_and_data_to_set(&mut self) -> HashSet<(u64, Bytes)> {
        self.blocks
            .iter()
            .zip(self.data.iter())
            .map(|(block, data)| (*block, data.clone()))
            .collect()
    }

    /// Gets the hash of the wallet state sync record.
    ///
    /// This method computes a deterministic hash of the wallet state sync record
    /// by combining the peer ID, UUID, and all data chunks. The hash is computed
    /// using SHA-256 and can be used for verification and comparison of sync
    /// records across network nodes.
    ///
    /// # Returns
    ///
    /// A 32-byte SHA-256 hash as a `Vec<u8>`.
    pub fn get_hash(&self) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(self.peer_id.as_slice());
        hasher.update(self.uuid.as_slice());
        for data_chunk in &self.data {
            hasher.update(data_chunk);
        }
        hasher.finalize().to_vec()
    }
}

/// Converts a `uuid::Uuid` to a `UuidID`.
///
/// This utility function converts a standard UUID (16 bytes) to a 32-byte
/// `UuidID` by padding with zeros. This is necessary because the storage
/// system uses 32-byte identifiers for consistency with other hash-based
/// identifiers in the system.
///
/// # Parameters
///
/// * `uuid` - The UUID to convert
///
/// # Returns
///
/// A 32-byte `UuidID` with the UUID bytes in the first 16 bytes and zeros
/// in the remaining 16 bytes.
pub fn uuid_to_b256(uuid: uuid::Uuid) -> UuidID {
    let mut uuid_fixed_bytes = [0u8; 32];
    uuid_fixed_bytes[0..16].copy_from_slice(uuid.as_bytes());
    uuid_fixed_bytes.into()
}

#[cfg(test)]
mod tests {
    use reth_primitives::hex;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn wallet_state_sync_record_test() {
        let uuid = Uuid::new_v4();
        let uuid_fixed_bytes = uuid_to_b256(uuid);
        let peer_id = PeerID::random();
        let data_chunk = Bytes::from(vec![1, 2, 3]);
        let chunks_count = 1;
        let block_number = 100;
        let wallet_state_sync_record = WalletStateSyncRecord {
            uuid: uuid_fixed_bytes.into(),
            peer_id,
            data: vec![data_chunk.clone()],
            blocks: vec![block_number],
            chunks_count,
        };
        assert_eq!(wallet_state_sync_record.get_uuid(), uuid_fixed_bytes);
        assert_eq!(wallet_state_sync_record.get_peer_id(), peer_id);
        assert_eq!(wallet_state_sync_record.get_data(), [data_chunk]);
        assert_eq!(wallet_state_sync_record.get_blocks(), [block_number]);
        assert_eq!(wallet_state_sync_record.get_chunks_count(), chunks_count);
        assert_eq!(wallet_state_sync_record.size(), 32 + 32 + 3 + 8);

        let hash = wallet_state_sync_record.get_hash();
        assert_eq!(hex::encode(hash).len(), 64);
    }
}
