use super::peg_contract::{EssentialPeginData, PeginData, PegoutWithId};
use crate::SealedBlockWithSenders;

/// Sealed block with pegin and pegout data
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedBlockWithPeg {
    /// Sealed block with senders
    block: SealedBlockWithSenders,
    /// Pegins
    pegins: Vec<(PeginData, Vec<EssentialPeginData>)>,
    /// Pegouts
    pegouts: Vec<PegoutWithId>,
}

impl SealedBlockWithPeg {
    /// Create a new `SealedBlockWithPeg`
    pub const fn new(
        block: SealedBlockWithSenders,
        pegins: Vec<(PeginData, Vec<EssentialPeginData>)>,
        pegouts: Vec<PegoutWithId>,
    ) -> Self {
        Self { block, pegins, pegouts }
    }

    /// Returns the block
    pub const fn block(&self) -> &SealedBlockWithSenders {
        &self.block
    }

    /// Pegins
    pub fn pegins(&self) -> &[(PeginData, Vec<EssentialPeginData>)] {
        self.pegins.as_slice()
    }

    /// Pegouts
    pub fn pegouts(&self) -> &[PegoutWithId] {
        self.pegouts.as_slice()
    }
}
