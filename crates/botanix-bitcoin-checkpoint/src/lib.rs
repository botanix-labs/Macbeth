//! Bitcoin checkpoint management module.
//!
//! This module provides functionality to track and verify Bitcoin blocks as checkpoints
//! for cross-chain verification and network consensus on Bitcoin state. It maintains
//! a window of blocks with different confirmation depths to ensure consistency and
//! availability of Bitcoin data.

mod chain;
mod checkpoint;
mod error;
mod stream;
mod syncer;

pub use chain::BitcoinCheckpointsChain;
pub use checkpoint::BitcoinCheckpoint;
pub use error::BitcoinCheckpointError;
pub use stream::DummyHashBlockStream;
pub use syncer::{BitcoinCheckpointsChainSynchronizer, BitcoinHashBlockStream};
