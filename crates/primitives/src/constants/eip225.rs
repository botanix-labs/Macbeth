//! EIP-225: Clique Proof-of-Authority consensus protocol.
use revm_primitives::U256;

/// The number of blocks to reset pending votes.
pub const EPOCH_LENGTH: u64 = 30;

/// Minimum difference between two consecutive block’s timestamps.
pub const BLOCK_PERIOD: u64 = 1000;

/// Magic nonce number 0xffffffffffffffff to vote on adding a new signer. Used in PoA
pub const NONCE_AUTH: u64 = 0xffffffffffffffff;

/// Magic nonce number 0x0000000000000000 to vote on removing a signer. Used in PoA
pub const NONCE_DROP: u64 = 0x0000000000000000;

/// Block score (difficulty) for blocks containing out-of-turn signatures.
/// TODO (armins) these will be removed
pub const DIFF_NOTURN: U256 = U256::ZERO;

/// Block score (difficulty) for blocks containing in-turn signatures.
/// TODO (armins) these will be removed
pub const DIFF_INTURN: U256 = U256::ZERO;

/// Block score (difficulty) for blocks containing no votes
pub const DIFF_NOVOTE: U256 = U256::ZERO;
