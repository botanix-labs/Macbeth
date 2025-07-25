mod builder;
mod manager;
#[cfg(feature = "test-utils")]
pub mod test_utils;
mod vote_tracker;

pub use builder::ActivationManagerBuilder;
pub use manager::*;
pub use reth_primitives::Address;
pub use vote_tracker::*;
