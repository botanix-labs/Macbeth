use crate::models::{WalletSweepSession, WalletSweepSessionId};
use reth_storage_errors::provider::ProviderResult;

#[auto_impl::auto_impl(&, Arc, Box)]
pub trait WalletSweepSessionReader: Send + Sync {
    fn get_wallet_sweep_session(
        &self,
    ) -> ProviderResult<Option<(WalletSweepSessionId, WalletSweepSession)>>;

    fn is_wallet_sweep_session_exists(
        &self,
        session_id: WalletSweepSessionId,
    ) -> ProviderResult<bool>;
}

#[auto_impl::auto_impl(&, Arc, Box)]
pub trait WalletSweepSessionWriter: Send + Sync {
    fn update_wallet_sweep_session(
        &self,
        session: WalletSweepSession,
    ) -> ProviderResult<WalletSweepSessionId>;
}
