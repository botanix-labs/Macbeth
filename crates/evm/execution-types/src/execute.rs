use reth_primitives::{
    botanix::peg_contract::{PeginData, PegoutWithId},
    Request, U256,
};
use revm::db::BundleState;

/// A helper type for ethereum block inputs that consists of a block and the total difficulty.
#[derive(Debug)]
pub struct BlockExecutionInput<'a, Block> {
    /// The block to execute.
    pub block: &'a Block,
    /// The total difficulty of the block.
    pub total_difficulty: U256,
    /// Whether to disable peging validation. This MUST be set to `true` during
    /// the `process_proposal` stage, but can be disabled during the
    /// `finalize_block` stage.
    pub disable_pegin_validation: bool,
}

impl<'a, Block> BlockExecutionInput<'a, Block> {
    /// Creates a new input.
    pub const fn new(block: &'a Block, total_difficulty: U256, disable_pegin_validation: bool) -> Self {
        Self { block, total_difficulty, disable_pegin_validation }
    }
}

impl<'a, Block> From<(&'a Block, U256)> for BlockExecutionInput<'a, Block> {
    fn from((block, total_difficulty): (&'a Block, U256)) -> Self {
        Self::new(block, total_difficulty, false)
    }
}

/// The output of an ethereum block.
///
/// Contains the state changes, transaction receipts, and total gas used in the block.
///
/// TODO(mattsse): combine with `ExecutionOutcome`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockExecutionOutput<T> {
    /// The changed state of the block after execution.
    pub state: BundleState,
    /// All the receipts of the transactions in the block.
    pub receipts: Vec<T>,
    /// All the EIP-7685 requests of the transactions in the block.
    pub requests: Vec<Request>,
    /// The total gas used by the block.
    pub gas_used: u64,
    /// Total block fees
    pub total_block_fees: u128,
    /// Pegins
    pub pegins: Vec<PeginData>,
    /// Pegouts
    pub pegouts: Vec<PegoutWithId>,
}
