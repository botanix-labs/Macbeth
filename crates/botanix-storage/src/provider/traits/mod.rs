//! # Provider Traits
//!
//! This module defines the abstract interfaces for all database operations in the Botanix
//! storage system. These traits provide a clean API surface that separates the concerns
//! of different storage domains.
//!
//! ## Trait Categories
//!
//! ### Factory Traits
//! - [`DatabaseProviderFactoryRO`]: Creates read-only database providers
//! - [`DatabaseProviderFactoryRW`]: Creates read-write database providers
//!
//! ### Domain-Specific Traits
//! - [`SnapshotReader`] / [`SnapshotWriter`]: Blockchain snapshot management
//! - [`StagedHeaderReader`] / [`StagedHeaderWriter`]: Header staging with pegin/pegout data
//! - [`WalletStateSyncReader`] / [`WalletStateSyncWriter`]: Wallet state synchronization
//! - [`ActivationManagerReader`] / [`ActivationManagerWriter`]: Network upgrade management

mod activation_manager;
mod factory;
mod foundation;
mod runtime_transitions;
mod snapshot;
mod staged_header;
mod wallet_state_sync;

pub use activation_manager::*;
pub use factory::*;
pub use foundation::*;
pub use runtime_transitions::*;
pub use snapshot::*;
pub use staged_header::*;
pub use wallet_state_sync::*;
