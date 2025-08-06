//! # Data Models
//!
//! This module contains all the core data structures used by the Botanix storage system.
//! These models represent the persistent data entities that are stored in the database
//! and exchanged between network peers.
//!
//! ## Module Organization
//!
//! - **activation_manager**: Network upgrade voting and version management models
//! - **snapshot**: Blockchain snapshot and chunk data structures
//! - **staged_header**: Headers with associated pegin/pegout transaction data
//! - **wallet_sync**: Wallet state synchronization coordination models
//! - **wallet_sweep**: Wallet sweep session models for emergency fund recovery
//!
//! All models implement the necessary traits for:
//! - Serialization/deserialization with `serde`
//! - Compact encoding for efficient storage with `reth-codecs`
//! - Arbitrary instance generation for testing (when enabled)
//!
//! ## Database Compression
//!
//! The models are configured with database compression support through the
//! `impl_compression_for_compact!` macro, which provides efficient storage
//! by leveraging the Reth compact encoding system.

mod activation_manager;
mod snapshot;
mod staged_header;
mod wallet_sweep;
mod wallet_sync;

pub use activation_manager::*;
pub use snapshot::*;
pub use staged_header::*;
pub use wallet_sweep::*;
pub use wallet_sync::*;

use reth_codecs::Compact;
use reth_db_api::{
    impl_compression_for_compact,
    table::{Compress, Decompress},
};

impl_compression_for_compact!(
    Snapshot,
    SnapshotChunk,
    SnapshotSync,
    HeaderWithPegs,
    WalletStateSyncRecord,
    WalletSweepSession,
);
