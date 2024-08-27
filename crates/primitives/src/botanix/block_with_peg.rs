use crate::SealedBlockWithSenders;

use super::peg_contract::{PeginData, PegoutData};


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedBlockWithPeg {
    block: SealedBlockWithSenders,
    pegins: Vec<PeginData>,
    pegouts: Vec<PegoutData>,
}

impl SealedBlockWithPeg {
    pub fn new(
        block: SealedBlockWithSenders,
        pegins: Vec<PeginData>,
        pegouts: Vec<PegoutData>,
    ) -> Self {
        Self {
            block,
            pegins,
            pegouts,
        }
    }
    
    /// Returns the block
    pub fn block(&self) -> &SealedBlockWithSenders {
        &self.block
    }

    /// 
    pub fn pegins(&self) -> &[PeginData] {
        &self.pegins
    }

    pub fn pegouts(&self) -> &[PegoutData] {
        &self.pegouts
    }
}