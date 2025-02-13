use reth_primitives::{BlockNumber, SealedBlock, SealedHeader, U256};

/// The block response
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum BlockResponse {
    /// Full block response (with transactions or ommers)
    Full(SealedBlock),
    /// The empty block response
    Empty(SealedHeader),
}

impl BlockResponse {
    /// Return the reference to the response header
    pub const fn header(&self) -> &SealedHeader {
        match self {
            Self::Full(block) => &block.header,
            Self::Empty(header) => header,
        }
    }

    /// Calculates a heuristic for the in-memory size of the [`BlockResponse`].
    #[inline]
    pub fn size(&self) -> usize {
        match self {
            Self::Full(block) => SealedBlock::size(block),
            Self::Empty(header) => SealedHeader::size(header),
        }
    }

    /// Return the block number
    pub fn block_number(&self) -> BlockNumber {
        self.header().as_ref().number
    }

    /// Return the reference to the response header
    pub fn difficulty(&self) -> U256 {
        match self {
            Self::Full(block) => block.as_ref().difficulty,
            Self::Empty(header) => header.as_ref().difficulty,
        }
    }
}
