use crate::models::RuntimeVersion;
use reth_primitives::BlockNumber;
use reth_storage_errors::provider::ProviderResult;

#[auto_impl::auto_impl(&, Arc, Box)]
/// Provides read and write operations for tracking runtime version transitions
/// across blocks encountered during finalization.
pub trait RuntimeTransitionsReadWrite: Send + Sync {
    /// Records a runtime upgrade at the specified block height.
    ///
    /// This method tracks runtime version transitions by storing the highest
    /// runtime version seen at each block height. If a version lower than or
    /// equal to the currently stored highest version is provided, it will be
    /// ignored.
    ///
    /// Returns `true` if the provided version is the highest seen and has been
    /// recorded.
    fn insert_runtime_upgrade_version(
        &self,
        height: BlockNumber,
        version: RuntimeVersion,
    ) -> ProviderResult<bool>;
    /// Retrieves the complete history of recorded runtime version transitions.
    fn get_runtime_versions(&self) -> ProviderResult<Vec<(BlockNumber, RuntimeVersion)>>;
    /// Retrieves the most recent (highest) runtime version that has been recorded.
    fn get_last_runtime_version(&self) -> ProviderResult<Option<RuntimeVersion>>;
}
