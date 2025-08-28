use crate::{
    models::{WalletSweepSession, WalletSweepSessionId},
    tables::WalletSweepSessions,
    BotanixDatabaseProvider, BotanixDatabaseProviderRW, WalletSweepSessionReader,
    WalletSweepSessionWriter,
};
use reth_db_api::{
    cursor::DbCursorRO,
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_storage_errors::provider::ProviderResult;

impl<TX: DbTx> WalletSweepSessionReader for BotanixDatabaseProvider<TX> {
    fn get_wallet_sweep_session(
        &self,
    ) -> ProviderResult<Option<(WalletSweepSessionId, WalletSweepSession)>> {
        self.tx
            .cursor_read::<WalletSweepSessions>()?
            .walk(None)?
            .next()
            .transpose()
            .map_err(Into::into)
    }

    fn is_wallet_sweep_session_exists(&self) -> ProviderResult<bool> {
        let session = self.get_wallet_sweep_session()?;

        Ok(session.is_some())
    }
}

impl<DB: Database> WalletSweepSessionReader for BotanixDatabaseProviderRW<DB> {
    fn get_wallet_sweep_session(
        &self,
    ) -> ProviderResult<Option<(WalletSweepSessionId, WalletSweepSession)>> {
        self.0.get_wallet_sweep_session()
    }

    fn is_wallet_sweep_session_exists(&self) -> ProviderResult<bool> {
        self.0.is_wallet_sweep_session_exists()
    }
}

impl<DB: Database> WalletSweepSessionWriter for BotanixDatabaseProviderRW<DB> {
    fn update_wallet_sweep_session(
        &self,
        session: WalletSweepSession,
    ) -> ProviderResult<WalletSweepSessionId> {
        self.tx.clear::<WalletSweepSessions>()?;

        let session_id = session.calculate_id();

        self.tx.put::<WalletSweepSessions>(session_id, session)?;

        Ok(session_id)
    }

    fn clear_wallet_sweep_session(&self) -> ProviderResult<Option<WalletSweepSessionId>> {
        let session_id = self.get_wallet_sweep_session()?.map(|(id, _)| id);

        self.tx.clear::<WalletSweepSessions>()?;

        Ok(session_id)
    }
}
