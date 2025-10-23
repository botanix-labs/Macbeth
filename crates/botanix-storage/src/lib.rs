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

use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// Wrapper for any type that implements `Serialize` and `Deserialize` to be
/// used as a generic MDBX key. This uses regular CBOR encoding without any
/// special optimization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnoptimizedKey<T>(pub T);

impl<T> From<T> for UnoptimizedKey<T> {
    fn from(value: T) -> Self {
        UnoptimizedKey(value)
    }
}

/// Keys are de/serialized using the `Encode` and `Decode` traits.
impl<T> reth_db_api::table::Encode for UnoptimizedKey<T>
where
    T: Send + Sync + Sized + std::fmt::Debug + Serialize,
{
    type Encoded = Vec<u8>;

    fn encode(self) -> Self::Encoded {
        let mut bytes = vec![];
        ciborium::into_writer(&self.0, &mut bytes).expect("writer must not fail");
        bytes
    }
}

/// Keys are de/serialized using the `Encode` and `Decode` traits.
impl<T> reth_db_api::table::Decode for UnoptimizedKey<T>
where
    T: Send + Sync + Sized + std::fmt::Debug + DeserializeOwned,
{
    fn decode<B: AsRef<[u8]>>(value: B) -> Result<Self, reth_db::DatabaseError> {
        ciborium::from_reader::<T, _>(value.as_ref())
            .map(UnoptimizedKey)
            .map_err(|_| reth_db::DatabaseError::Decode)
    }
}

// TODO: Actually need this?
impl<T> PartialEq for UnoptimizedKey<T>
where
    T: Send + Sync + Sized + std::fmt::Debug + Serialize,
{
    fn eq(&self, other: &Self) -> bool {
        let self_bytes = {
            let mut bytes = vec![];
            ciborium::into_writer(&self.0, &mut bytes).expect("serialization must not fail");
            bytes
        };

        let other_bytes = {
            let mut bytes = vec![];
            ciborium::into_writer(&other.0, &mut bytes).expect("serialization must not fail");
            bytes
        };

        self_bytes == other_bytes
    }
}

// TODO: Actually need this?
impl<T> Eq for UnoptimizedKey<T> where T: Send + Sync + Sized + std::fmt::Debug + Serialize {}

// TODO: Actually need this?
impl<T> PartialOrd for UnoptimizedKey<T>
where
    T: Send + Sync + Sized + std::fmt::Debug + Serialize,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// TODO: Actually need this?
impl<T> Ord for UnoptimizedKey<T>
where
    T: Send + Sync + Sized + std::fmt::Debug + Serialize,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let self_bytes = {
            let mut bytes = vec![];
            ciborium::into_writer(&self.0, &mut bytes).expect("serialization must not fail");
            bytes
        };

        let other_bytes = {
            let mut bytes = vec![];
            ciborium::into_writer(&other.0, &mut bytes).expect("serialization must not fail");
            bytes
        };

        self_bytes.cmp(&other_bytes)
    }
}

/// Wrapper for any type that implements `Serialize` and `Deserialize` to be
/// used as a generic MDBX value. This uses regular CBOR encoding without any
/// special optimization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnoptimizedValue<T>(pub T);

impl<T> From<T> for UnoptimizedValue<T> {
    fn from(value: T) -> Self {
        UnoptimizedValue(value)
    }
}

/// Values are de/serialized (“compressed”) using the `Compress` and
/// `Decompress` traits.
impl<T> reth_db_api::table::Compress for UnoptimizedValue<T>
where
    T: Send + Sync + Sized + std::fmt::Debug + Serialize,
{
    type Compressed = Vec<u8>;

    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(self, buf: &mut B) {
        // TODO: Can this be improved?
        let mut bytes = vec![];
        ciborium::into_writer(&self.0, &mut bytes).expect("writer must not fail");
        buf.put(bytes.as_slice());
    }
}

/// Values are de/serialized (“compressed”) using the `Compress` and
/// `Decompress` traits.
impl<T> reth_db_api::table::Decompress for UnoptimizedValue<T>
where
    T: Send + Sync + Sized + std::fmt::Debug + DeserializeOwned,
{
    fn decompress<B: AsRef<[u8]>>(value: B) -> Result<Self, reth_db::DatabaseError> {
        ciborium::from_reader::<T, _>(value.as_ref())
            .map(UnoptimizedValue)
            .map_err(|_| reth_db::DatabaseError::Decode)
    }
}
