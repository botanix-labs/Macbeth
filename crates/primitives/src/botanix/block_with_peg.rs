use super::peg_contract::{PeginData, PegoutWithId};
use crate::SealedBlockWithSenders;
use serde::{Deserialize, Serialize};

/// Sealed block with pegin and pegout data
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SealedBlockWithPeg {
    /// Sealed block with senders
    block: SealedBlockWithSenders,
    /// Pegins
    pegins: Vec<PeginData>,
    /// Pegouts
    pegouts: Vec<PegoutWithId>,
}

impl SealedBlockWithPeg {
    /// Create a new `SealedBlockWithPeg`
    pub const fn new(
        block: SealedBlockWithSenders,
        pegins: Vec<PeginData>,
        pegouts: Vec<PegoutWithId>,
    ) -> Self {
        Self { block, pegins, pegouts }
    }

    /// Returns the block
    pub const fn block(&self) -> &SealedBlockWithSenders {
        &self.block
    }

    /// Pegins
    pub fn pegins(&self) -> &[PeginData] {
        &self.pegins
    }

    /// Pegouts
    pub fn pegouts(&self) -> &[PegoutWithId] {
        &self.pegouts
    }
}
