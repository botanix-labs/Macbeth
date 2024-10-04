use reth_revm::primitives::FixedBytes;

/// Trait that provides a random source of 32 bytes.
pub trait RandomSource {
    /// Returns a random source.
    fn random_source(&self) -> FixedBytes<32>;
}

/// Struct that provides a random source of 32 bytes.
pub struct RandomSourceProvider;

impl RandomSource for RandomSourceProvider {
    // TODO: Implement a proper random source.
    fn random_source(&self) -> FixedBytes<32> {
        FixedBytes::default()
    }
}
