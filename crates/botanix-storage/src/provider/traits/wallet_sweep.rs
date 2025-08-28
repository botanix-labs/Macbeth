use crate::models::{WalletSweepSession, WalletSweepSessionId};
use reth_storage_errors::provider::ProviderResult;

/// Trait for reading wallet sweep session data from storage.
///
/// This trait provides methods to retrieve wallet sweep session information
/// from the underlying storage system.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait WalletSweepSessionReader: Send + Sync {
    /// Retrieves the current wallet sweep session from storage.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some((session_id, session)))` if a session exists,
    /// or `Ok(None)` if no session is currently stored.
    fn get_wallet_sweep_session(
        &self,
    ) -> ProviderResult<Option<(WalletSweepSessionId, WalletSweepSession)>>;

    /// Checks if a wallet sweep session with the given ID exists in storage.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The ID of the session to check for existence.
    ///
    /// # Returns
    ///
    /// Returns `Ok(true)` if the session exists, `Ok(false)` otherwise.
    fn is_wallet_sweep_session_exists(&self) -> ProviderResult<bool>;
}

/// Trait for writing wallet sweep session data to storage.
///
/// This trait provides methods to update and persist wallet sweep session information
/// in the underlying storage system.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait WalletSweepSessionWriter: Send + Sync {
    /// Updates or creates a wallet sweep session in storage.
    ///
    /// # Arguments
    ///
    /// * `session` - The wallet sweep session to store.
    ///
    /// # Returns
    ///
    /// Returns the ID of the stored session on success.
    fn update_wallet_sweep_session(
        &self,
        session: WalletSweepSession,
    ) -> ProviderResult<WalletSweepSessionId>;

    /// Removes the current wallet sweep session from storage.
    ///
    /// # Returns
    ///
    /// Returns the ID of the stored session on success.
    fn clear_wallet_sweep_session(&self) -> ProviderResult<Option<WalletSweepSessionId>>;
}
