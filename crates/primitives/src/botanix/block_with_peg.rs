use crate::SealedBlockWithSenders;

use super::peg_contract::{PeginData, PegoutData};

/// Sealed block with pegin and pegout data
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedBlockWithPeg {
    /// Sealed block with senders
    block: SealedBlockWithSenders,
    /// Pegins
    pegins: Vec<PeginData>,
    /// Pegouts
    pegouts: Vec<PegoutData>,
}

impl SealedBlockWithPeg {
    /// Create a new SealedBlockWithPeg
    pub fn new(
        block: SealedBlockWithSenders,
        pegins: Vec<PeginData>,
        pegouts: Vec<PegoutData>,
    ) -> Self {
        Self { block, pegins, pegouts }
    }

    /// Returns the block
    pub fn block(&self) -> &SealedBlockWithSenders {
        &self.block
    }

    /// Pegins
    pub fn pegins(&self) -> &[PeginData] {
        &self.pegins
    }

    /// Pegouts
    pub fn pegouts(&self) -> &[PegoutData] {
        &self.pegouts
    }
}
