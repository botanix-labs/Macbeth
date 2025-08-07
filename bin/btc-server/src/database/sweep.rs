use crate::database::{Db, Error};
use botanix_storage::{
    models::{WalletSweepSession, WalletSweepSessionId},
    tables::Compress,
};
use sled::{IVec, Subscriber};

impl Db {
    pub fn update_wallet_sweep_session(
        &self,
        session: WalletSweepSession,
    ) -> Result<WalletSweepSessionId, Error> {
        self.wallet_sweep_session.clear()?;

        let session_id = session.calculate_id();

        self.wallet_sweep_session.insert(session_id, session.compress())?;

        Ok(session_id)
    }

    pub fn subscribe_to_wallet_sweep_session_updates(&self) -> Subscriber {
        self.wallet_sweep_session.watch_prefix(Vec::new())
    }

    pub fn get_wallet_sweep_session_bytes(&self) -> Result<Option<(IVec, IVec)>, Error> {
        self.wallet_sweep_session.iter().next().transpose().map_err(Into::into)
    }
}
