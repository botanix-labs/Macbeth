use crate::models::HeaderWithPegs;
use reth_primitives::B256;
use reth_storage_errors::provider::ProviderResult;

/// Trait for managing staged headers. This is used to store pegins and pegouts
/// extracted from a finalized block, making sure that none of the pegins or
/// pegouts are lost.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait StagedHeaderReader: Send + Sync {
    /// Retrieve all staged headers.
    ///
    /// Returns all staged headers currently stored in the database. Each staged header
    /// contains a blockchain header along with its associated pegin and pegout data.
    /// This method is used to process pending Bitcoin bridge operations.
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<(B256, HeaderWithPegs)>)` - A vector of tuples where:
    ///   - First element is the header hash (B256)
    ///   - Second element is the header with pegin/pegout data
    /// * `Err(ProviderError)` - If there was a database error
    ///
    /// # Performance
    ///
    /// This method loads all staged headers into memory. For systems with many
    /// staged headers, consider implementing pagination or filtering.
    fn get_staged_headers(&self) -> ProviderResult<Vec<(B256, HeaderWithPegs)>>;
}

/// Trait for managing staged headers. This is used to store pegins and pegouts
/// extracted from a finalized block, making sure that none of the pegins or
/// pegouts are lost.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait StagedHeaderWriter: Send + Sync {
    /// Insert a staged header with the given header hash.
    ///
    /// Stores a header along with its associated pegin and pegout data in the
    /// staged headers table. This is typically called after a block is finalized
    /// and Bitcoin bridge operations need to be preserved for later processing.
    ///
    /// # Parameters
    ///
    /// * `id` - The header hash that uniquely identifies this staged header
    /// * `header` - The header with associated pegin and pegout data
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the header was successfully inserted
    /// * `Err(ProviderError)` - If there was a database error
    fn insert_staged_header(&self, id: B256, header: HeaderWithPegs) -> ProviderResult<()>;
    /// Remove a staged header by its header hash.
    ///
    /// Removes a staged header from the database after it has been processed.
    /// This is typically called after all pegin and pegout operations in the
    /// header have been successfully handled.
    ///
    /// # Parameters
    ///
    /// * `id` - The header hash of the staged header to remove
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - If the header was found and successfully removed
    /// * `Ok(false)` - If no header was found with the given hash
    /// * `Err(ProviderError)` - If there was a database error
    fn remove_staged_header(&self, id: B256) -> ProviderResult<bool>;
}
