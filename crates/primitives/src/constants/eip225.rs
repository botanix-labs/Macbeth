//! EIP-225: Clique Proof-of-Authority consensus protocol.
use crate::U128;
use ruint::uint;

/// The number of blocks to reset pending votes.
pub const EPOCH_LENGTH: u64 = 30000;

/// Minimum difference between two consecutive block’s timestamps.
pub const BLOCK_PERIOD: u64 = 15;

/// Magic nonce number 0xffffffffffffffff to vote on adding a new signer. Used in PoA
pub const NONCE_AUTH: u64 = 0xffffffffffffffff;

/// Magic nonce number 0x0000000000000000 to vote on removing a signer. Used in PoA
pub const NONCE_DROP: u64 = 0x0000000000000000;

/// Block score (difficulty) for blocks containing out-of-turn signatures.
pub const DIFF_NOTURN: U128 = uint!(1_U128);

/// Block score (difficulty) for blocks containing in-turn signatures.
pub const DIFF_INTURN: U128 = uint!(2_U128);

/// Block score (difficulty) for blocks containing no signatures.
pub const DIFF_NOVOTE: U128 = uint!(0_U128);
