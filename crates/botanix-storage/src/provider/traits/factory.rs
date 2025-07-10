use crate::{
    SnapshotReader, SnapshotWriter, StagedHeaderReader, StagedHeaderWriter, WalletStateSyncReader,
    WalletStateSyncWriter,
};
use reth_storage_errors::provider::ProviderResult;

pub trait DatabaseProviderFactoryRO {
    type Provider: WalletStateSyncReader + SnapshotReader + StagedHeaderReader;
    fn provider(&self) -> ProviderResult<Self::Provider>;
}

pub trait DatabaseProviderFactoryRW {
    type Provider: WalletStateSyncWriter
        + WalletStateSyncReader
        + SnapshotReader
        + SnapshotWriter
        + StagedHeaderWriter
        + StagedHeaderReader;
    fn provider_rw(&self) -> ProviderResult<Self::Provider>;
}
