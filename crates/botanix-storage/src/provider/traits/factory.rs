use crate::{
    SnapshotReader, SnapshotWriter, StagedHeaderReader, StagedHeaderWriter, WalletStateSyncReader,
    WalletStateSyncWriter,
};
use reth_storage_errors::provider::ProviderResult;

/// Factory trait for creating read-only database providers.
///
/// This trait provides a clean way to create database providers that support
/// read-only operations across all Botanix storage domains. The returned provider
/// implements all the necessary reader traits for accessing stored data.
pub trait DatabaseProviderFactoryRO {
    /// The type of provider created by this factory.
    ///
    /// Must implement all reader traits for accessing Botanix storage data.
    type Provider: WalletStateSyncReader + SnapshotReader + StagedHeaderReader;

    /// Creates a new read-only database provider.
    ///
    /// # Returns
    ///
    /// A `ProviderResult` containing the database provider or an error if
    /// the provider could not be created.
    fn provider(&self) -> ProviderResult<Self::Provider>;
}

/// Factory trait for creating read-write database providers.
///
/// This trait provides a clean way to create database providers that support
/// both read and write operations across all Botanix storage domains. The returned
/// provider implements all necessary reader and writer traits.
///
/// ## Transaction Management
///
/// Read-write providers typically manage database transactions and require
/// explicit commit operations to persist changes.
pub trait DatabaseProviderFactoryRW {
    /// The type of provider created by this factory.
    ///
    /// Must implement all reader and writer traits for accessing and modifying
    /// Botanix storage data.
    type Provider: WalletStateSyncWriter
        + WalletStateSyncReader
        + SnapshotReader
        + SnapshotWriter
        + StagedHeaderWriter
        + StagedHeaderReader;

    /// Creates a new read-write database provider.
    ///
    /// # Returns
    ///
    /// A `ProviderResult` containing the database provider or an error if
    /// the provider could not be created.
    fn provider_rw(&self) -> ProviderResult<Self::Provider>;
}
