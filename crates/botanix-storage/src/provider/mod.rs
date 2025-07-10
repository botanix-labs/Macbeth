//! # Database Provider System
//!
//! This module implements the provider pattern for database operations in the Botanix storage
//! system. It provides a clean abstraction layer over the underlying database implementation,
//! allowing for both read-only and read-write operations through trait-based interfaces.
//!
//! ## Architecture
//!
//! The provider system is organized into two main components:
//!
//! - [`traits`]: Abstract interfaces defining the available operations
//! - [`database`]: Concrete implementations of the traits using the Reth database
//!
//! ## Usage Examples
//!
//! ### Read-Only Operations
//!
//! ```rust,ignore
//! use botanix_storage::{DatabaseProviderFactoryRO, SnapshotReader};
//! use reth_db::{init_db, mdbx::DatabaseArguments};
//!
//! let database = init_db("./db/path", DatabaseArguments::default())?;
//! let database = Arc::new(botanix_database);
//!
//! let provider_factory = DatabaseProviderFactoryRO::new(database);
//!
//! // Create a read-only provider
//! let provider = provider_factory.provider()?;
//!
//! // Read snapshot data
//! let snapshots = provider.get_snapshots()?;
//! ```
//!
//! ### Read-Write Operations
//!
//! ```rust,ignore
//! use botanix_storage::{DatabaseProviderFactoryRO, SnapshotReader};
//! use reth_db::{init_db, mdbx::DatabaseArguments};
//!
//! let database = init_db("./db/path", DatabaseArguments::default())?;
//! let database = Arc::new(botanix_database);
//!
//! let provider_factory = DatabaseProviderFactoryRW::new(database);
//!
//! let provider = provider_factory.provider_rw()?;
//! let snapshot_id = provider.create_new_snapshot(block_number, block_hash)?;
//! provider.commit()?;
//! ```

mod database;
mod traits;

pub use database::*;

pub use traits::*;
