use crate::database::Db;
use crate::pegout_id::PegoutId;
use crate::wallet::psbt::validate::data_provider::{
    PendingPegoutData, PsbtDataProvider, PsbtDataProviderError,
};
use crate::wallet::util::VerifyingKeyExt;

#[async_trait::async_trait]
impl PsbtDataProvider for Db {
    fn pending_pegout_data(
        &self,
        id: &PegoutId,
    ) -> Result<Option<PendingPegoutData>, PsbtDataProviderError> {
        let pegout_data = self
            .get_pending_pegout(id)?
            .map(|request| PendingPegoutData { amount: request.value, script_pubkey: request.spk });

        Ok(pegout_data)
    }

    fn contains_finalized_pegout(&self, id: &PegoutId) -> Result<bool, PsbtDataProviderError> {
        Ok(self.get_finalized_pegout(id)?.is_some())
    }

    async fn aggregate_public_key(
        &self,
    ) -> Result<bitcoin::secp256k1::PublicKey, PsbtDataProviderError> {
        let public_key_package = self
            .get_public_key_package()?
            .ok_or(PsbtDataProviderError::AggregatePublicKeyNotFound)?;

        // TxOut scriptpubkey should be scriptpubkey derived from aggregated public key
        let public_key = public_key_package.verifying_key().to_secp_pk()?;

        Ok(public_key)
    }
}
