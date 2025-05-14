use crate::storage::memory::Storage;
use crate::utils::PsbtValidationError;
use btcserverlib::pegout_id::PegoutId;
use btcserverlib::wallet::psbt::validate::data_provider::{
    PendingPegoutData, PsbtDataProvider, PsbtDataProviderError,
};
use reth_primitives::botanix::mint_validation::try_parse_burn_event;
use reth_primitives::botanix::peg_contract::PegoutData;
use reth_provider::ReceiptProvider;
use secp256k1::PublicKey;
use tracing::debug;

impl<EF, BF, DB> PsbtDataProvider for Storage<EF, BF, DB>
where
    DB: ReceiptProvider,
{
    fn pending_pegout_data(
        &self,
        id: &PegoutId,
    ) -> Result<Option<PendingPegoutData>, PsbtDataProviderError> {
        let Some(receipt) = self.client.receipt_by_hash(id.txid.into())? else {
            debug!("..");
            return Ok(None);
        };

        let Some(log) = receipt.logs.get(*id.idx as usize).cloned() else {
            debug!("..");
            return Ok(None);
        };

        let Some(PegoutData { amount, destination, .. }) =
            try_parse_burn_event(&log, self.btc_network)
        else {
            debug!("..");
            return Ok(None);
        };

        // .map_err(|e| {
        //     PsbtValidationError::FailedToValidatePsbtByIds(format!(
        //         "failed to parse burn event"
        //     ))
        // })?
        //     .ok_or_else(|| {
        //         PsbtValidationError::FailedToValidatePsbtByIds(format!(
        //             "failed to get pegout data from burn event"
        //         ))
        //     })?;
        //
        Ok(Some(PendingPegoutData { amount, script_pubkey: destination.script_pubkey() }))
    }

    fn contains_finalized_pegout(&self, id: &PegoutId) -> Result<bool, PsbtDataProviderError> {
        todo!()
    }

    async fn aggregate_public_key(&self) -> Result<PublicKey, PsbtDataProviderError> {
        self.read()
            .await
            .aggregate_public_key
            .ok_or(PsbtDataProviderError::AggregatePublicKeyNotFound)
    }
}
