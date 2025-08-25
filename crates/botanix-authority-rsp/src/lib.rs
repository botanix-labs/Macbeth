//! Module for providing random sources.
use std::fmt::Debug;

use reth_revm::primitives::FixedBytes;

/// Trait that provides a random source of 32 bytes.
pub trait RandomSource: Debug + Clone {
    /// Returns a random source.
    fn random_source(&self) -> FixedBytes<32>;
}

/// Struct that provides a random source of 32 bytes.
#[derive(Debug, Clone, Default)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_source_provider_debug() {
        let provider = RandomSourceProvider::new();
        let debug_output = format!("{:?}", provider);
        assert!(!debug_output.is_empty());
    }

    #[test]
    fn test_random_source_returns_fixed_bytes() {
        let provider = RandomSourceProvider::new();
        let random_bytes = provider.random_source();
        // verify that we get an all-zero byte array
        assert_eq!(random_bytes, FixedBytes::default());
    }

    #[test]
    fn test_random_source_default_is_zeros() {
        // test that the default implementation returns all zeros
        let provider = RandomSourceProvider::new();
        let random_bytes = provider.random_source();

        let expected = [0u8; 32];
        assert_eq!(random_bytes.as_slice(), &expected);
    }

    // Mock implementation for testing
    #[derive(Debug, Clone)]
    struct MockRandomSource {
        value: [u8; 32],
    }

    impl MockRandomSource {
        fn new(value: [u8; 32]) -> Self {
            Self { value }
        }
    }

    impl RandomSource for MockRandomSource {
        fn random_source(&self) -> FixedBytes<32> {
            FixedBytes::from_slice(&self.value)
        }
    }

    #[test]
    fn test_trait_implementation() {
        // Test that we can use the trait implementation
        let mock_value = [42u8; 32];
        let mock_source = MockRandomSource::new(mock_value);

        let random_bytes = mock_source.random_source();

        assert_eq!(random_bytes.as_slice(), &mock_value);
    }
}
