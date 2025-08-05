use crate::database::{Db, Error};

impl Db {
    pub fn init_wallet_sweep_session(&self) {
        // let mut bytes = Vec::new();
        // ciborium::into_writer(&dkg_round2_package, &mut bytes).map_err(Error::CiboriumWrite)?;

        // self.wallet_sweep_session.insert();
    }
}

pub enum WalletSweepSession {
    Init,
    Accepted,
}
