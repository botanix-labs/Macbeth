//! Bitcoin checkpoint representation.
//!
//! A checkpoint represents a specific block in the Bitcoin blockchain that can be used
//! for cross-chain verification and network consensus on Bitcoin state.

use bitcoin::block::BlockHash as BitcoinBlockHash;
use bitcoin::block::Header as BitcoinHeader;
use std::fmt::Display;

/// Represents a Bitcoin block that serves as a checkpoint.
///
/// A checkpoint contains the block header, its height in the blockchain,
/// and a precomputed hash for quick comparison.
#[derive(PartialEq, Debug, Clone)]
pub struct BitcoinCheckpoint {
    /// Bitcoin block header.
    pub header: BitcoinHeader,
    /// Block height in the Bitcoin chain.
    pub height: u32,
    /// Bitcoin block hash.
    /// To avoid hashing the header multiple times, we cache it here.
    pub hash: BitcoinBlockHash,
}

impl BitcoinCheckpoint {
    /// Creates a new Bitcoin checkpoint from a block header and height.
    ///
    /// The hash is computed from the header and stored for later use.
    ///
    /// # Parameters
    /// * `header` - The Bitcoin block header
    /// * `height` - The block height in the Bitcoin blockchain
    ///
    /// # Returns
    /// A new `BitcoinCheckpoint` instance
    pub fn new(header: BitcoinHeader, height: u32) -> Self {
        let hash = header.block_hash();
        Self { header, height, hash }
    }
}

impl Display for BitcoinCheckpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BitcoinCheckpoint {{ height: {}, hash: {} }}", self.height, self.hash)
    }
}
