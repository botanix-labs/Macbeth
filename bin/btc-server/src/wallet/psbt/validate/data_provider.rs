use crate::pegout_id::PegoutId;
use bitcoin::{Amount, ScriptBuf};

#[derive(Debug, thiserror::Error)]
pub enum PsbtDataProviderError {
    #[error("BtcServer database error: {0}")]
    BtcServerDatabaseError(#[from] crate::database::Error),
    #[error("Aggregate public key not found")]
    AggregatePublicKeyNotFound,
    #[error("Can't convert verifying key to secp256k1 public key: {0}")]
    VerifyingKeyToSecpPkError(#[from] crate::wallet::util::VerifyingKeyExtError),
}

#[async_trait::async_trait]
pub trait PsbtDataProvider {
    fn pending_pegout_data(
        &self,
        id: &PegoutId,
    ) -> Result<Option<PendingPegoutData>, PsbtDataProviderError>;

    fn contains_finalized_pegout(&self, id: &PegoutId) -> Result<bool, PsbtDataProviderError>;

    async fn aggregate_public_key(
        &self,
    ) -> Result<bitcoin::secp256k1::PublicKey, PsbtDataProviderError>;
}

pub struct PendingPegoutData {
    pub amount: Amount,
    pub script_pubkey: ScriptBuf,
}
