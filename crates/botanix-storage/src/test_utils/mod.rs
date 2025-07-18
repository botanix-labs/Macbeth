//! # Test Utilities
//!
//! This module provides testing utilities for the Botanix storage system.
//! It includes mock implementations, test helpers, and utilities for creating
//! test data and scenarios.
//!
//! ## Available when the `test-utils` feature is enabled
//!
//! This module is only available when the `test-utils` feature is enabled in
//! the Cargo.toml configuration. This ensures that test utilities are not
//! included in production builds.
//!
//! ## Components
//!
//! - [`provider`]: Database providers for testing
//!
//! ## Usage
//!
//! ```rust,ignore
//! #[cfg(test)]
//! mod tests {
//!     use botanix_storage::test_utils::*;
//!
//!     #[test]
//!     fn test_snapshot_operations() {
//!         let provider = create_test_provider_factory();
//!         // Test operations...
//!     }
//! }
//! ```

mod provider;

pub use provider::*;
