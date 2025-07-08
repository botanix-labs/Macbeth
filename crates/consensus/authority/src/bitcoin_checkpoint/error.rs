//! Error types for Bitcoin checkpoint operations.
//!
//! This module defines errors that can occur during Bitcoin checkpoint creation,
//! management, and synchronization.

use bitcoin::block::BlockHash as BitcoinBlockHash;

/// Errors that can occur in Bitcoin checkpoint operations.
#[derive(thiserror::Error, Debug)]
pub enum BitcoinCheckpointError {
    /// Attempted to add a block that doesn't connect to the current chain.
    #[error(
        "Block from other chain added: expected previous block hash {expected_prev_block_hash}, found {received_prev_block_hash}"
    )]
    StaleBlockAdded {
        /// The hash we expected based on our latest checkpoint
        expected_prev_block_hash: BitcoinBlockHash,
        /// The hash we received from the new checkpoint
        received_prev_block_hash: BitcoinBlockHash,
    },

    /// An error occurred while calling the Bitcoin RPC.
    #[error("Bitcoin RPC call {procedure_name} failed on checkpoint sync: {error}")]
    SyncRpcError {
        /// The underlying JSON-RPC error
        error: botanix_btc_wallet::bitcoind::JsonRPCError,
        /// Name of the procedure that failed
        procedure_name: String,
    },

    /// Strong confirmation depth was set to zero.
    #[error("Strong confirmation depth must be greater than zero")]
    ZeroStrongConfirmationDepth,

    /// Weak checkpoints count exceeds strong confirmation depth.
    #[error("Weak checkpoints count {weak_checkpoints_count} must be less than strong confirmation depth {strong_confirmation_depth}")]
    WeakCheckpointsCountTooBig {
        /// The weak checkpoints count that was provided
        weak_checkpoints_count: usize,
        /// The strong confirmation depth that was provided
        strong_confirmation_depth: usize,
    },

    /// Chain configuration parameters would cause numeric overflow.
    #[error("Chain configuration values are too big: strong_confirmation_depth={strong_confirmation_depth}, historical_checkpoints_count={historical_checkpoints_count}, weak_checkpoints_count={weak_checkpoints_count}")]
    ChainParamsTooLarge {
        /// The strong confirmation depth that was provided
        strong_confirmation_depth: usize,
        /// The historical checkpoints count that was provided
        historical_checkpoints_count: usize,
        /// The weak checkpoints count that was provided
        weak_checkpoints_count: usize,
    },
}
