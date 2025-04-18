use std::collections::HashSet;

use reth_db::models::{PeerID, UuidID, WalletStateSyncRecord};
use reth_primitives::Bytes;
use reth_storage_errors::provider::ProviderResult;

/// WalletStateSyncReader
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait WalletStateSyncReader: Send + Sync {
    /// Get all state sync records
    fn get_state_sync_records(&self) -> ProviderResult<Vec<WalletStateSyncRecord>>;

    /// Get all state sync record peer ids
    fn get_state_sync_record_peer_ids(&self) -> ProviderResult<Vec<PeerID>>;

    /// Get state sync record by peer id
    fn get_state_sync_record_by_peer_id(
        &self,
        peer_id: PeerID,
    ) -> ProviderResult<Option<WalletStateSyncRecord>>;

    /// Get state sync recors count
    fn get_state_sync_records_count(&self) -> ProviderResult<usize>;

    /// Get miniumm superset
    /// Returns a tuple of a boolean indicating if the minimum superset is found and a hashset of
    /// bytes
    fn get_minimum_superset(
        &self,
        min_required_criterion: u64,
    ) -> ProviderResult<(bool, HashSet<Bytes>)>;
}

/// WalletStateSyncWriter
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait WalletStateSyncWriter: Send + Sync {
    /// Create new state sync record
    fn create_new_state_sync_record(
        &self,
        uuid: UuidID,
        peer_id: PeerID,
        chunks_count: u64,
        data: Option<Vec<Bytes>>,
    ) -> ProviderResult<PeerID>;

    /// Append data to state sync record
    fn append_data_to_state_sync_record(
        &self,
        peer_id: PeerID,
        data: Vec<Bytes>,
    ) -> ProviderResult<()>;

    /// Remove state sync record by peer_id
    fn remove_state_sync_record_per_peer_id(&self, peer_id: PeerID) -> ProviderResult<()>;

    /// Removes all state sync records
    fn remove_all_state_sync_records(&self) -> ProviderResult<()>;
}
