use crate::models::{PeerID, UuidID, WalletStateSyncRecord};
use reth_primitives::Bytes;
use reth_storage_errors::provider::ProviderResult;
use std::collections::HashSet;

/// Trait for reading wallet state synchronization data from the database.
///
/// This trait provides read-only access to wallet state synchronization records,
/// which coordinate the synchronization of wallet states across network peers.
/// The wallet state sync system ensures that all peers maintain consistent
/// wallet information for proper Bitcoin pegin/pegout operations.
///
/// ## Synchronization Model
///
/// - **Peer-based**: Each peer maintains its own synchronization record
/// - **Chunked Data**: Wallet state is synchronized in chunks for efficiency
/// - **Superset Logic**: Minimum supersets are calculated for consensus
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait WalletStateSyncReader: Send + Sync {
    /// Get all state sync records
    ///
    /// Retrieves all wallet state synchronization records from the database.
    /// This method returns records for all peers that are participating in
    /// wallet state synchronization.
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<WalletStateSyncRecord>)` - A vector of all sync records
    /// * `Err(ProviderError)` - If there was a database error
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

    /// Get minimum superset
    ///
    /// Calculates the minimum superset of wallet state data across all peer
    /// synchronization records. This is used to determine the consensus wallet
    /// state when multiple peers have contributed different data sets.
    ///
    /// The algorithm finds the smallest set of (block, data) pairs that appears
    /// in at least the minimum required number of peer records. This ensures
    /// that the wallet state reflects data that has been validated by multiple peers.
    ///
    /// # Parameters
    ///
    /// * `min_required_criterion` - The minimum number of peer records that must contain a data
    ///   pair for it to be included in the superset
    ///
    /// # Returns
    ///
    /// * `Ok((true, HashSet<(u64, Bytes)>))` - If a valid superset was found:
    ///   - First element is `true` indicating success
    ///   - Second element contains the minimum superset of (block, data) pairs
    /// * `Ok((false, HashSet<(u64, Bytes)>))` - If no valid superset was found:
    ///   - First element is `false` indicating failure
    ///   - Second element is an empty or partial set
    /// * `Err(ProviderError)` - If there was a database error
    fn get_minimum_superset(
        &self,
        min_required_criterion: u64,
    ) -> ProviderResult<(bool, HashSet<(u64, Bytes)>)>;
}

/// Trait for writing wallet state synchronization data to the database.
///
/// This trait provides write access to wallet state synchronization records,
/// enabling the creation, updating, and management of peer synchronization state.
/// Writers can create new sync records, append data, and clean up obsolete records.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait WalletStateSyncWriter: Send + Sync {
    /// Create new state sync record
    fn create_new_state_sync_record(
        &self,
        uuid: UuidID,
        peer_id: PeerID,
        chunks_count: u64,
        data: Option<Vec<(u64, Bytes)>>,
    ) -> ProviderResult<PeerID>;

    /// Append data to state sync record
    fn append_data_to_state_sync_record(
        &self,
        peer_id: PeerID,
        data: Vec<(u64, Bytes)>,
    ) -> ProviderResult<()>;

    /// Remove state sync record by peer_id
    fn remove_state_sync_record_per_peer_id(&self, peer_id: PeerID) -> ProviderResult<()>;

    /// Removes all state sync records
    fn remove_all_state_sync_records(&self) -> ProviderResult<()>;
}
