use crate::database::{Db, Error};
use botanix_storage::models::WalletSweepSession;
use reth_db_api::table::Compress;
use sled::{IVec, Subscriber};

impl Db {
    pub fn update_wallet_sweep_session(&self, session: WalletSweepSession) -> Result<(), Error> {
        let session_id = session.calculate_id();

        self.wallet_sweep_session.insert(session_id, session.compress())?;

        Ok(())
    }

    pub fn subscribe_to_wallet_sweep_session_updates(&self) -> Subscriber {
        self.wallet_sweep_session.watch_prefix(Vec::new())
    }

    pub fn get_wallet_sweep_session_bytes(&self) -> Result<Option<(IVec, IVec)>, Error> {
        self.wallet_sweep_session.iter().next().transpose().map_err(Into::into)
    }
}
