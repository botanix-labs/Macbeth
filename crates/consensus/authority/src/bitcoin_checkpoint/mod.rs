mod chain;
mod checkpoint;
mod error;
mod syncer;

pub use chain::BitcoinCheckpointsChain;
pub use checkpoint::BitcoinCheckpoint;
pub use error::BitcoinCheckpointError;
pub use syncer::BitcoinCheckpointsChainSynchronizer;

// TODO: overflows
// TODO: logs
// TODO: comments
// TODO: doc blocks
// TODO: tests
