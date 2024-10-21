//! Module for providing random sources.
use std::fmt::Debug;

use reth_revm::primitives::FixedBytes;

/// Trait that provides a random source of 32 bytes.
pub trait RandomSource: Debug {
    /// Returns a random source.
    fn random_source(&self) -> FixedBytes<32>;
}

/// Struct that provides a random source of 32 bytes.
#[derive(Debug, Default)]
pub struct RandomSourceProvider;

impl RandomSourceProvider {
    /// Creates a new `RandomSourceProvider`.
    pub fn new() -> Self {
        Self
    }
}

impl RandomSource for RandomSourceProvider {
    // TODO: Implement a proper random source.
    fn random_source(&self) -> FixedBytes<32> {
        FixedBytes::default()
    }
}
