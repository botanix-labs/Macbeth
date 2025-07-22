//! # Botanix Storage
//!
//! A storage implementation for the Botanix blockchain that provides persistent storage
//! for snapshots, staged headers, wallet state synchronization, and activation manager data.
//!
//! This crate provides a comprehensive storage layer built on top of the Reth database
//! infrastructure, specifically designed for the Botanix blockchain's unique requirements
//! including Bitcoin pegins/pegouts, consensus snapshots, and network upgrade management.
//!
//! ## Key Features
//!
//! - **Snapshot Management**: Storage for blockchain snapshots with chunked data support
//! - **Staged Headers**: Persistence of headers with associated pegin/pegout data
//! - **Wallet State Sync**: Coordination of wallet state synchronization across peers
//! - **Activation Manager**: Network upgrade voting and activation tracking
//! - **Database Abstractions**: Clean provider pattern for read/write operations
//!
//! ## Architecture
//!
//! The crate is organized into several key modules:
//!
//! - [`models`]: Core data structures for all storage entities
//! - [`tables`]: Database table definitions and schemas
//! - Provider system: Database provider traits and implementations
//! - Test utilities: Testing utilities (feature-gated)
//!
//! ## Usage
//!
//! The storage layer follows a provider pattern where operations are performed through
//! trait-based interfaces:
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
//! ## Database Tables
//!
//! The storage system uses the following primary tables:
//!
//! - `Snapshots`: Snapshot metadata and chunk references
//! - `Chunks`: Individual snapshot chunk data
//! - `StagedHeader`: Headers with pegin/pegout data
//! - `WalletStateSyncs`: Wallet synchronization records
//! - `BlockSnapshots`: Block number to snapshot ID mappings
//! - `SnapshotSyncs`: Snapshot synchronization progress tracking

pub mod models;
mod provider;
pub mod tables;

#[cfg(feature = "test-utils")]
pub mod test_utils;

pub use provider::*;
