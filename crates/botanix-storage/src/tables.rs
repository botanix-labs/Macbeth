//! # Database Table Definitions
//!
//! This module defines all the database tables used by the Botanix storage system.
//! Each table represents a specific data domain and defines the key-value mapping
//! for efficient storage and retrieval.
//!
//! ## Table Overview
//!
//! The Botanix storage system uses the following tables:
//!
//! ### Snapshot Tables
//! - [`Snapshots`]: Maps snapshot IDs to snapshot metadata
//! - [`Chunks`]: Maps chunk IDs to individual chunk data
//! - [`BlockSnapshots`]: Maps block numbers to snapshot IDs
//! - [`ChunkBlocks`]: Maps chunk IDs to their block numbers
//! - [`SnapshotSyncs`]: Tracks snapshot synchronization progress
//!
//! ### Header Tables
//! - [`StagedHeader`]: Maps block hashes to headers with pegin/pegout data
//!
//! ### Wallet Sync Tables
//! - [`WalletStateSyncs`]: Maps peer IDs to wallet state sync records
//!
//! ## Usage with Reth
//!
//! These tables integrate with the Reth database infrastructure, providing
//! type-safe access to the underlying MDBX database with automatic serialization
//! and deserialization of complex data structures.

use super::models::*;
use reth_db::{tables, TableType, TableViewer};
use reth_primitives::{BlockNumber, B256};
use std::fmt;

tables! {
    /// Store snapshot id to snapshot data.
    ///
    /// This table maintains the primary snapshot metadata including height,
    /// associated chunk IDs, block IDs, and block hash. Each snapshot
    /// represents a point-in-time state of the blockchain.
    table Snapshots<Key = SnapshotId, Value = Snapshot>;

    /// Store wallet state sync record id to wallet state sync data.
    ///
    /// This table tracks wallet state synchronization across network peers,
    /// storing the coordination data needed to ensure all peers maintain
    /// consistent wallet state information.
    table WalletStateSyncs<Key = PeerID, Value = WalletStateSyncRecord>;

    /// Store staged headers, used to persist pegins and pegouts after
    /// finalizing a block.
    ///
    /// Staged headers contain Bitcoin pegin/pegout transaction data that
    /// has been extracted from finalized blocks. This staging mechanism
    /// ensures no pegin or pegout data is lost during block processing.
    table StagedHeader<Key = B256, Value = HeaderWithPegs>;

    /// Store chunk id to chunk data.
    ///
    /// Chunks represent segments of snapshot data, allowing for efficient
    /// storage and transmission of large snapshot datasets. Each chunk
    /// contains a range of blocks with their associated transaction data.
    table Chunks<Key = ChunkId, Value = SnapshotChunk>;

    /// Stores block number to snapshot id.
    ///
    /// This mapping table provides efficient lookup from block numbers
    /// to their associated snapshot IDs, enabling fast historical queries.
    table BlockSnapshots<Key = BlockNumber, Value = SnapshotId>;

    /// Stores the chunk to Block ids
    ///
    /// Maps chunk IDs to their corresponding block numbers, providing
    /// efficient reverse lookup for chunk-to-block relationships.
    table ChunkBlocks<Key = ChunkId, Value = BlockNumber>;

    /// Table used when syncing snapshots.
    ///
    /// Tracks the progress of snapshot synchronization operations,
    /// including total chunks, applied chunks, and synchronization state.
    table SnapshotSyncs<Key = SnapshotSyncId, Value = SnapshotSync>;
}
