use std::time::SystemTime;

use bdk::{miniscript::psbt::Error as PsbtError, psbt::PsbtUtils};
use bitcoin::{
    hashes::sha256,
    psbt::{ExtractTxError, Psbt},
    taproot::SigFromSliceError,
    BlockHash, FeeRate, TxOut,
};
use bitcoincore_rpc::{json::EstimateMode, RpcApi};
use frost_secp256k1_tr as frost;
use rand::thread_rng;
use reth_btc_wallet::{
    psbt::{PsbtExt, PsbtInputExt, PsbtOutputExt},
    transaction::CalculateSighashError,
};

use crate::{
    coordinator::CoordinatorError,
    database,
    pegout_id::PegoutId,
    util::{validate_psbt, ROUND1, ROUND1_TRANSITION},
    App, Error,
};

const SATS_PER_KWU_DIVISOR: u64 = 4;

#[derive(Debug)]
pub enum SigningError {
    Round1(SigningRound1Error),
    Round2(SigningRound2Error),
    Finalize(SigningFinalizeError),
    Abort(SigningAbortError),
}

impl From<SigningError> for Error {
    fn from(e: SigningError) -> Error {
        match e {
            SigningError::Round1(e) => Error::Signing(SigningError::Round1(e)),
            SigningError::Round2(e) => Error::Signing(SigningError::Round2(e)),
            SigningError::Finalize(e) => Error::Signing(SigningError::Finalize(e)),
            SigningError::Abort(e) => Error::Signing(SigningError::Abort(e)),
        }
    }
}

#[derive(Debug, Error)]
pub enum SigningRound1Error {
    #[error("already in signing session")]
    AlreadyInSigningSession,
    #[error("missing key package")]
    MissingKeyPackage,
    #[error("invalid number of signing nonces requested")]
    InvalidNumberOfNoncesRequested,
    #[error("invalid signing package: {0}")]
    InvalidSigningPackage(&'static str),
    #[error("internal DB error")]
    Db(#[from] database::Error),
    #[error("failed to add signing commits to psbt")]
    FailedToAddSigningCommitsToPsbt(
        #[from] reth_btc_wallet::psbt::PsbtToSigningPackageConversionError,
    ),
    #[error("failed to get smart estimate fee rate")]
    FailedToGetEstimateSmartFeeRate,
    #[error("fee rate difference is too great")]
    FeeRateDifferenceTooGreat,
    #[error("failed to validate psbt: {0}")]
    FailedToValidatePsbt(#[from] crate::util::ValidatePSBTError),
    #[error("extract tx error: {0}")]
    ExtractTxError(#[from] ExtractTxError),
    #[error("failed to get fee rate from psbt")]
    FailedToGetFeeRateFromPsbt,
}

#[derive(Debug, Error)]
pub enum SigningRound2Error {
    #[error("missing key package")]
    MissingKeyPackage,
    #[error("invalid signing package: {0}")]
    InvalidSigningPackage(&'static str),
    #[error("internal FROST error: {0}")]
    FrostError(#[from] frost::Error),
    #[error("missing round 1 signing nonces")]
    MissingRound1SigningNonce,
    #[error("internal DB error")]
    Db(#[from] database::Error),
    #[error("signer not found in signing package at index: {0}")]
    SignerNotFound(usize),
    #[error("Failed to calculate sighash: {0}")]
    FailedToCalculateSighash(#[from] CalculateSighashError),
    #[error("Failed parse out to sign package: {0}")]
    PsbtToSigningPackageConversionError(
        #[from] reth_btc_wallet::psbt::PsbtToSigningPackageConversionError,
    ),
    #[error("failed to validate psbt: {0}")]
    FailedToValidatePsbt(#[from] crate::util::ValidatePSBTError),
    #[error("extract tx error: {0}")]
    ExtractTxError(#[from] ExtractTxError),
}
impl From<SigningRound1Error> for SigningError {
    fn from(e: SigningRound1Error) -> Self {
        SigningError::Round1(e)
    }
}

impl From<SigningRound2Error> for SigningError {
    fn from(e: SigningRound2Error) -> Self {
        SigningError::Round2(e)
    }
}

#[derive(Debug, Error)]
pub enum SigningFinalizeError {
    #[error("missing key package")]
    MissingKeyPackage,
    #[error("too many witness items")]
    TooManyWitnessItems,
    #[error("PSBT finalization failed : {0:?}")]
    PsbtFinalizationFailed(Vec<PsbtError>),
    #[error("Taproot Signature validation error: {0}")]
    TaprootSignatureValidationError(#[from] bitcoin::taproot::TaprootError),
    #[error("internal DB error")]
    DbError(#[from] database::Error),
    #[error("sig from slice error: {0}")]
    SigFromSliceError(#[from] SigFromSliceError),
    #[error("extract tx error: {0}")]
    ExtractTxError(#[from] ExtractTxError),
    #[error("coordinator internal error: {0}")]
    CoordinatorError(#[from] CoordinatorError),
    #[error("missing pending pegout: {0}")]
    MissingPendingPegout(PegoutId),
    #[error("psbt pegout missing pegout id")]
    MissingPsbtPegout,
    #[error("psbt to signing package conversion error: {0}")]
    PsbtToSigningPackageConversionError(
        #[from] reth_btc_wallet::psbt::PsbtToSigningPackageConversionError,
    ),
    #[error("FROST error: {0}")]
    FrostError(#[from] frost::Error),
}

#[derive(Debug, Error)]
pub enum SigningAbortError {
    #[error("missing key package")]
    MissingKeyPackage,
    #[error("internal DB error")]
    DbError(#[from] database::Error),
}

impl<BitcoindClient> App<BitcoindClient>
where
    BitcoindClient: RpcApi + Send + Sync + 'static,
{
    pub(crate) async fn abort_signing(&self) -> Result<(), SigningAbortError> {
        self.db.get_key_package()?.ok_or(SigningAbortError::MissingKeyPackage)?;

        // Clear the signing nonces
        let mut nonces_lock = self.frost_round1_nonces.lock().await;
        nonces_lock.take();
        assert!(nonces_lock.is_none());

        Ok(())
    }

    pub(crate) async fn get_round1_signing_package(
        &self,
        psbt: &mut Psbt,
        _signing_session_id: &[u8; 32],
    ) -> Result<(), SigningRound1Error> {
        self.db.get_key_package()?.ok_or(SigningRound1Error::MissingKeyPackage)?;
        // Check if have already provided nonces for the current session
        let mut nonces_lock = self.frost_round1_nonces.lock().await;
        if nonces_lock.is_some() {
            return Err(SigningRound1Error::AlreadyInSigningSession);
        }
        // check fee is within acceptable range
        let psbt_fee_rate =
            psbt.fee_rate().ok_or(SigningRound1Error::FailedToGetFeeRateFromPsbt)?;
        // fetch fee rate from bitcoind
        let fee_res = self.bitcoind_client.estimate_smart_fee(1, Some(EstimateMode::Conservative));

        let mut fee_rate = self.fall_back_fee_rate;
        if let Ok(fee) = fee_res {
            if let Some(f) = fee.fee_rate {
                fee_rate = FeeRate::from_sat_per_kwu(f.to_sat() / SATS_PER_KWU_DIVISOR);
            }
        }
        let diff = fee_rate.to_sat_per_kwu().abs_diff(psbt_fee_rate.to_sat_per_kwu()) as f64;
        // convert config field to percentage
        let acceptable_fee_rate_diff = ((self.config.fee_rate_diff_percentage as f64) / 100.0) *
            fee_rate.to_sat_per_kwu() as f64;

        if diff > acceptable_fee_rate_diff {
            debug!("[signer] fee rate difference is too great");
            debug!("[signer] acceptable fee rate difference: {:?}", acceptable_fee_rate_diff);
            debug!("[signer] fee rate difference: {:?}", diff);
            debug!("[signer] fee rate from bitcoind/fallback: {:?}", fee_rate);
            debug!("[signer] fee rate from psbt: {:?}", psbt_fee_rate);
            debug!(
                "[signer] config fee rate diff percentage: {:?}",
                self.config.fee_rate_diff_percentage
            );
            return Err(SigningRound1Error::FeeRateDifferenceTooGreat);
        }

        let tx = psbt.clone().extract_tx()?;
        // Validate the psbt
        for (index, input) in psbt.inputs.iter().enumerate() {
            if input.witness_utxo.is_none() {
                return Err(SigningRound1Error::InvalidSigningPackage("witness_utxo is missing"));
            }

            // Check if input exists in db
            let ot = tx.input.get(index).expect("valid index").previous_output;
            let db_utxo = self.db.get_utxo(ot)?;
            if db_utxo.is_none() {
                return Err(SigningRound1Error::InvalidSigningPackage("UTXO not found in DB"));
            }
        }
        // Validate PSBT
        validate_psbt(psbt, ROUND1, self.min_signers, &self.db)?;
        let num_inputs = psbt.inputs.len();

        let key_package =
            self.db.get_key_package()?.ok_or(SigningRound1Error::MissingKeyPackage)?;
        // Get our secret package
        let secret = key_package.signing_share();
        let mut nonces = vec![];

        let mut rng = thread_rng();
        // Order here is important for both the signer and coordinator
        // Each nonce pair is commitment to a input of the tx
        // When the signing package is produced the signer should be careful to
        // Verify that the nonce pairs are in the same order as the inputs
        for i in 0..num_inputs {
            let nonce_pkg = frost::round1::commit(secret, &mut rng);
            psbt.inputs[i].set_signing_commitment(self.identifier, &nonce_pkg.1);
            nonces.push(nonce_pkg);
        }

        // Save signing nonces in memory
        let signing_nonces =
            nonces.iter().map(|nonce| (nonce.0.clone(), nonce.1)).collect::<Vec<_>>();
        nonces_lock.replace(signing_nonces);
        Ok(())
    }

    pub(crate) async fn get_round2_signing_package(
        &self,
        psbt: &mut Psbt,
    ) -> Result<(), SigningRound2Error> {
        // Important note here is that we never reuse the same nonce pairs for a different signing
        // request Should always generate new ones or if we are in a signing session refuse
        // to provide new ones
        let key_package =
            self.db.get_key_package()?.ok_or(SigningRound2Error::MissingKeyPackage)?;

        // Validate PSBT
        validate_psbt(psbt, ROUND1_TRANSITION, self.min_signers, &self.db)?;

        let tx = psbt.clone().extract_tx()?;
        let num_inputs = tx.input.len();
        let mut signing_packages = psbt.signing_packages()?;

        // Get signing nonces from round 1
        let mut nonces_lock = self.frost_round1_nonces.lock().await;
        let signing_nonces =
            nonces_lock.clone().ok_or(SigningRound2Error::MissingRound1SigningNonce)?;

        if signing_nonces.len() != num_inputs {
            return Err(SigningRound2Error::InvalidSigningPackage(
                "Number of signing nonces does not match number of inputs",
            ));
        }
        for (index, signing_package) in signing_packages.iter().enumerate() {
            // Check if this signer is in the signing set
            // This should also implicitly validate the psbt
            // In other words this signer would have never provided nonce pairs if the psbt was not
            // valid from round 1
            let signing_commitments = signing_package.signing_commitments();
            if !signing_commitments.contains_key(&self.identifier) {
                return Err(SigningRound2Error::SignerNotFound(index));
            }
            let our_sc = signing_commitments.get(&self.identifier).expect("valid index");
            let our_nonce = signing_nonces.get(index).expect("valid index");
            if our_sc != &our_nonce.1 {
                return Err(SigningRound2Error::InvalidSigningPackage(
                    "Invalid nonce pair for this signer",
                ));
            }
        }

        // Get a partial signature for each input
        for (index, (signing_package, psbt_in)) in
            signing_packages.iter_mut().zip(psbt.inputs.iter_mut()).enumerate()
        {
            let sigs = frost::round2::sign(
                signing_package,
                &signing_nonces.get(index).expect("valid index").0,
                &key_package,
            )?;
            psbt_in.set_partial_signature(self.identifier, &sigs);
        }

        // Clear the signing nonces
        // This finalizes the signing session
        nonces_lock.take();
        Ok(())
    }

    pub(crate) async fn finalize_signer(
        &self,
        finalized_psbt: Psbt,
    ) -> Result<Psbt, SigningFinalizeError> {
        let mut finalized_psbt = finalized_psbt.clone();
        let _key_package =
            self.db.get_key_package()?.ok_or(SigningFinalizeError::MissingKeyPackage)?;
        let pk_package =
            self.db.get_public_key_package()?.ok_or(SigningFinalizeError::MissingKeyPackage)?;

        let signing_packages = finalized_psbt
            .signing_packages()
            .map_err(SigningFinalizeError::PsbtToSigningPackageConversionError)?;

        // Verify each inputs signature by aggregating and then verifying
        for (index, psbt_input) in finalized_psbt.inputs.iter_mut().enumerate() {
            let signing_package = signing_packages.get(index).expect("valid index").clone();
            let partial_sig = psbt_input.all_partial_signatures();
            let agg_sig = frost::aggregate(&signing_package, &partial_sig, &pk_package)?;

            // Verify signature
            if let Some(e) = psbt_input.eth_address() {
                pk_package.verifying_key().verify(
                    signing_package.message(),
                    &agg_sig,
                    Some(e.clone().as_slice()),
                )?;
            } else {
                pk_package.verifying_key().verify(signing_package.message(), &agg_sig, None)?;
            }
        }

        // Check all pending pegouts are being settled in this tx
        // TODO this should be checked at every step of the way. Put this check in validate_psbt
        let pending_pegouts = self.db.get_pending_pegouts()?;
        let pending_outputs = pending_pegouts
            .iter()
            .map(|p| (TxOut { value: p.value, script_pubkey: p.spk.clone() }, Some(p.id)))
            .collect::<Vec<(TxOut, Option<PegoutId>)>>();
        let pending_pegout_ids = pending_outputs
            .iter()
            .map(|o| o.1)
            .filter(|o| o.is_some())
            .map(|o| o.expect("valid pegout id"))
            .collect::<Vec<_>>();

        let mut psbt_pegout_ids: Vec<PegoutId> = vec![];
        for output in finalized_psbt.outputs.iter() {
            match output.pegout_id() {
                Some(id) => match PegoutId::from_bytes(&id) {
                    Ok(id) => psbt_pegout_ids.push(id),
                    Err(_) => return Err(SigningFinalizeError::MissingPsbtPegout),
                },
                None => return Err(SigningFinalizeError::MissingPsbtPegout),
            };
        }

        for pegout in pending_pegouts.iter() {
            if !psbt_pegout_ids.contains(&pegout.id) {
                return Err(SigningFinalizeError::MissingPendingPegout(pegout.id));
            }
        }

        let tx = finalized_psbt.clone().extract_tx()?;
        // We're finalizing it for the first time now.
        let tx_timestamp = SystemTime::now();
        self.add_tracked_tx(tx.clone(), &pending_pegouts, tx_timestamp).await?;
        self.db.remove_pending_pegout(&pending_pegout_ids)?;
        self.db.flush()?;

        // Lets broadcast the tx
        let tx_id = match self.bitcoind_client.send_raw_transaction(&tx) {
            Ok(tx_id) => Ok(Some(tx_id)),
            Err(err) => {
                let err_msg = err.to_string();
                if err_msg.contains("already in chain") {
                    Ok(None)
                } else {
                    error!("Failed to broadcast tx: {}", err);
                    Err(CoordinatorError::FailedToBroadcastTx(err))
                }
            }
        }?;

        if let Some(tx_id) = tx_id {
            info!("Broadcasted tx: {:?}", tx_id);
        } else {
            info!("Transaction already broadcasted and in pool");
        }

        Ok(finalized_psbt)
    }
}
