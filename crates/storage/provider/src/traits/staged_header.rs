use reth_db::models::HeaderWithPegs;
use reth_errors::ProviderResult;
use reth_primitives::B256;

/// Trait for managing staged headers. This is used to store pegins and pegouts
/// extracted from a finalized block, making sure that none of the pegins or
/// pegouts are lost.
#[auto_impl::auto_impl(&, Arc, Box)]
#[deprecated(note = "Please use `botanix-storage` create")]
pub trait StagedHeader: Send + Sync {
    /// Insert a staged header with the given header hash.
    #[deprecated(note = "Please use `botanix-storage` create")]
    fn insert_staged_header(&self, id: B256, header: HeaderWithPegs) -> ProviderResult<()>;
    /// Remove a staged header by its header hash.
    #[deprecated(note = "Please use `botanix-storage` create")]
    fn remove_staged_header(&self, id: B256) -> ProviderResult<bool>;
    /// Retrieve all staged headers.
    #[deprecated(note = "Please use `botanix-storage` create")]
    fn get_staged_headers(&self) -> ProviderResult<Vec<(B256, HeaderWithPegs)>>;
}
