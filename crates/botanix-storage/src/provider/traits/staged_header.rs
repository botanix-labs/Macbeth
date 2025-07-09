use crate::models::HeaderWithPegs;
use reth_primitives::B256;
use reth_storage_errors::provider::ProviderResult;

/// Trait for managing staged headers. This is used to store pegins and pegouts
/// extracted from a finalized block, making sure that none of the pegins or
/// pegouts are lost.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait StagedHeader: Send + Sync {
    /// Insert a staged header with the given header hash.
    fn insert_staged_header(&self, id: B256, header: HeaderWithPegs) -> ProviderResult<()>;
    /// Remove a staged header by its header hash.
    fn remove_staged_header(&self, id: B256) -> ProviderResult<bool>;
    /// Retrieve all staged headers.
    fn get_staged_headers(&self) -> ProviderResult<Vec<(B256, HeaderWithPegs)>>;
}
