use std::collections::HashSet;

use reth_db::models::{PeerID, UuidID, WalletStateSyncRecord};
use reth_primitives::Bytes;
use reth_storage_errors::provider::ProviderResult;

/// WalletStateSyncReader
#[auto_impl::auto_impl(&, Arc, Box)]
#[deprecated(note = "Please use `botanix-storage` create")]
pub trait WalletStateSyncReader: Send + Sync {
    /// Get all state sync records
    #[deprecated(note = "Please use `botanix-storage` create")]
    fn get_state_sync_records(&self) -> ProviderResult<Vec<WalletStateSyncRecord>>;

    /// Get all state sync record peer ids
    #[deprecated(note = "Please use `botanix-storage` create")]
    fn get_state_sync_record_peer_ids(&self) -> ProviderResult<Vec<PeerID>>;

    /// Get state sync record by peer id
    #[deprecated(note = "Please use `botanix-storage` create")]
    fn get_state_sync_record_by_peer_id(
        &self,
        peer_id: PeerID,
    ) -> ProviderResult<Option<WalletStateSyncRecord>>;

    /// Get state sync recors count
    #[deprecated(note = "Please use `botanix-storage` create")]
    fn get_state_sync_records_count(&self) -> ProviderResult<usize>;

    /// Get miniumm superset
    /// Returns a tuple of a boolean indicating if the minimum superset is found and a hashset of
    /// bytes
    #[deprecated(note = "Please use `botanix-storage` create")]
    fn get_minimum_superset(
        &self,
        min_required_criterion: u64,
    ) -> ProviderResult<(bool, HashSet<(u64, Bytes)>)>;
}

/// WalletStateSyncWriter
#[auto_impl::auto_impl(&, Arc, Box)]
#[deprecated(note = "Please use `botanix-storage` create")]
pub trait WalletStateSyncWriter: Send + Sync {
    /// Create new state sync record
    #[deprecated(note = "Please use `botanix-storage` create")]
    fn create_new_state_sync_record(
        &self,
        uuid: UuidID,
        peer_id: PeerID,
        chunks_count: u64,
        data: Option<Vec<(u64, Bytes)>>,
    ) -> ProviderResult<PeerID>;

    /// Append data to state sync record
    #[deprecated(note = "Please use `botanix-storage` create")]
    fn append_data_to_state_sync_record(
        &self,
        peer_id: PeerID,
        data: Vec<(u64, Bytes)>,
    ) -> ProviderResult<()>;

    /// Remove state sync record by peer_id
    #[deprecated(note = "Please use `botanix-storage` create")]
    fn remove_state_sync_record_per_peer_id(&self, peer_id: PeerID) -> ProviderResult<()>;

    /// Removes all state sync records
    #[deprecated(note = "Please use `botanix-storage` create")]
    fn remove_all_state_sync_records(&self) -> ProviderResult<()>;
}
