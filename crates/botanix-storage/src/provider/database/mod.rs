//! # Database Provider Implementation
//!
//! This module contains the concrete implementations of the provider traits using the
//! Reth database infrastructure. It provides the actual database operations for all
//! Botanix storage functionality.
//!
//! ## Core Components
//!
//! - [`BotanixDatabaseProvider`]: Main database provider implementation
//! - [`BotanixDatabaseProviderRO`]: Read-only database provider
//! - [`BotanixDatabaseProviderRW`]: Read-write database provider
//! - [`BotanixProviderFactory`]: Factory for creating provider instances
//!
//! ## Implementation Modules
//!
//! - [`factory`]: Factory implementations for creating providers
//! - [`provider`]: Core provider wrapper and transaction management
//! - [`snapshot`]: Snapshot-related database operations
//! - [`staged_header`]: Staged header database operations
//! - [`wallet_state_sync`]: Wallet state sync database operations
//!
//! ## Transaction Management
//!
//! The database providers handle both read-only and read-write transactions:
//!
//! - **Read-Only**: Provides efficient read access without locking
//! - **Read-Write**: Supports mutations with proper transaction semantics
//! - **Commit/Rollback**: Explicit transaction control for consistency
//!
//! ## Error Handling
//!
//! All database operations return `ProviderResult<T>` which wraps potential
//! database errors in a consistent error type for proper error propagation.

mod factory;
mod provider;
mod runtime_transitions;
mod snapshot;
mod staged_header;
mod wallet_state_sync;
mod wallet_sweep;

pub use factory::*;
pub use provider::*;
