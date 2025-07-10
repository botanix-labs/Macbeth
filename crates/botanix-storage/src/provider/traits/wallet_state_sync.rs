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
    ///
    /// Retrieves the peer IDs of all peers that have active wallet state
    /// synchronization records. This is useful for getting an overview of
    /// which peers are participating in wallet state synchronization.
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<PeerID>)` - A vector of peer IDs with active sync records
    /// * `Err(ProviderError)` - If there was a database error
    fn get_state_sync_record_peer_ids(&self) -> ProviderResult<Vec<PeerID>>;

    /// Get state sync record by peer id
    ///
    /// Retrieves the wallet state synchronization record for a specific peer.
    /// This is the most efficient way to access a peer's sync data when you
    /// know their peer ID.
    ///
    /// # Parameters
    ///
    /// * `peer_id` - The unique identifier of the peer whose sync record to retrieve
    ///
    /// # Returns
    ///
    /// * `Ok(Some(WalletStateSyncRecord))` - The sync record if found
    /// * `Ok(None)` - If no sync record exists for the given peer ID
    /// * `Err(ProviderError)` - If there was a database error
    fn get_state_sync_record_by_peer_id(
        &self,
        peer_id: PeerID,
    ) -> ProviderResult<Option<WalletStateSyncRecord>>;

    /// Get state sync records count
    ///
    /// Returns the total number of active wallet state synchronization records.
    /// This is useful for monitoring how many peers are currently participating
    /// in wallet state synchronization.
    ///
    /// # Returns
    ///
    /// * `Ok(usize)` - The number of active sync records
    /// * `Err(ProviderError)` - If there was a database error
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
    ///
    /// Creates a new wallet state synchronization record for a peer. This is
    /// typically called when a peer begins participating in wallet state sync.
    ///
    /// # Parameters
    ///
    /// * `uuid` - Unique session identifier for this synchronization session
    /// * `peer_id` - The peer identifier for the participating peer
    /// * `chunks_count` - Expected total number of chunks for this session
    /// * `data` - Optional initial data as (block_number, data) tuples
    ///
    /// # Returns
    ///
    /// * `Ok(PeerID)` - The peer ID of the created record
    /// * `Err(ProviderError)` - If there was a database error or the record already exists
    fn create_new_state_sync_record(
        &self,
        uuid: UuidID,
        peer_id: PeerID,
        chunks_count: u64,
        data: Option<Vec<(u64, Bytes)>>,
    ) -> ProviderResult<PeerID>;

    /// Append data to state sync record
    ///
    /// Appends additional data chunks to an existing wallet state synchronization
    /// record. This is used to incrementally build up the synchronized state data.
    ///
    /// # Parameters
    ///
    /// * `peer_id` - The peer ID whose sync record should be updated
    /// * `data` - Vector of (block_number, data) tuples to append
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the data was successfully appended
    /// * `Err(ProviderError)` - If there was a database error or the peer record doesn't exist
    fn append_data_to_state_sync_record(
        &self,
        peer_id: PeerID,
        data: Vec<(u64, Bytes)>,
    ) -> ProviderResult<()>;

    /// Remove state sync record by peer_id
    ///
    /// Removes a wallet state synchronization record for a specific peer.
    /// This is typically called when a peer is no longer participating in
    /// wallet state synchronization or when cleaning up completed sessions.
    ///
    /// # Parameters
    ///
    /// * `peer_id` - The peer ID whose sync record should be removed
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the record was successfully removed or didn't exist
    /// * `Err(ProviderError)` - If there was a database error
    fn remove_state_sync_record_per_peer_id(&self, peer_id: PeerID) -> ProviderResult<()>;

    /// Removes all state sync records
    ///
    /// Removes all wallet state synchronization records from the database.
    /// This is a destructive operation that clears all synchronization state.
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If all records were successfully removed
    /// * `Err(ProviderError)` - If there was a database error
    fn remove_all_state_sync_records(&self) -> ProviderResult<()>;
}
