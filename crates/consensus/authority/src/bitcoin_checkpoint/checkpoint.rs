use bitcoin::block::BlockHash as BitcoinBlockHash;
use bitcoin::block::Header as BitcoinHeader;
use std::fmt::Display;

#[derive(PartialEq, Debug, Clone)]
pub struct BitcoinCheckpoint {
    /// Bitcoin block header.
    pub header: BitcoinHeader,
    /// Block height in the Bitcoin chain.
    pub height: u32,
    /// To avoid hashing the header multiple times, we store the hash here.
    pub hash: BitcoinBlockHash,
}

impl BitcoinCheckpoint {
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
