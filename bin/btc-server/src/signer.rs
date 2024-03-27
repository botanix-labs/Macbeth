use crate::{
    database::{self, Error as DbError},
    util::{
        self, add_partial_signature_to_psbt, add_signing_commitments_to_psbt,
        convert_bdk_feerate_to_bitcoin, psbt_to_signing_packages, VerifyingKeyExt,
    },
    App, Error, SECP,
};
use bdk::miniscript::psbt::Error as PsbtError;

use bdk::psbt::PsbtUtils;
use bitcoin::{psbt::Psbt, FeeRate, TxOut};
use bitcoincore_rpc::json::EstimateMode;

use frost_secp256k1_tr as frost;
use miniscript::psbt::PsbtExt;
use rand::thread_rng;
use reth_btc_wallet::transaction::CalculateSighashError;

#[derive(Debug)]
pub enum SigningError {
    Round1(SigningRound1Error),
    Round2(SigningRound2Error),
    Finalize(SigningFinalizeError),
}

impl From<SigningError> for Error {
    fn from(e: SigningError) -> Error {
        match e {
            SigningError::Round1(e) => Error::Signing(SigningError::Round1(e)),
            SigningError::Round2(e) => Error::Signing(SigningError::Round2(e)),
            SigningError::Finalize(e) => Error::Signing(SigningError::Finalize(e)),
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
    FailedToAddSigningCommitsToPsbt(#[from] crate::util::PsbtToSigningPackageConversionError),
    #[error("failed to get smart estimate fee rate")]
    FailedToGetEstimateSmartFeeRate,
    #[error("fee rate difference is too great")]
    FeeRateDifferenceTooGreat,
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
    PsbtToSigningPackageConversionError(#[from] crate::util::PsbtToSigningPackageConversionError),
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
    TaprootSignatureValidationError(#[from] bitcoin::taproot::Error),
    #[error("internal DB error")]
    DbError(#[from] DbError),
}

impl App {
    pub(crate) async fn get_round1_signing_package(
        &self,
        mut psbt: &mut Psbt,
        _signing_session_id: &[u8; 32],
        bitcoind_client: &impl bitcoincore_rpc::RpcApi,
    ) -> Result<(), SigningRound1Error> {
        let num_inputs = psbt.inputs.len();
        if num_inputs == 0 || num_inputs > 15 {
            return Err(SigningRound1Error::InvalidNumberOfNoncesRequested);
        }
        // Check if have already provided nonces for the current session
        let mut nonces_lock = self.frost_round1_nonces.lock().await;
        if nonces_lock.is_some() {
            return Err(SigningRound1Error::AlreadyInSigningSession);
        }
        // check fee is within acceptable range
        let psbt_fee_rate =
            convert_bdk_feerate_to_bitcoin(psbt.fee_rate().expect("valid fee rate"));
        debug!("[signer] fee rate from psbt: {:?}", psbt_fee_rate);

        // fetch fee rate from bitcoind
        let fee_res = bitcoind_client.estimate_smart_fee(1, Some(EstimateMode::Conservative));

        let mut fee_rate = self.fall_back_fee_rate;
        if let Ok(fee) = fee_res {
            if let Some(f) = fee.fee_rate {
                fee_rate = FeeRate::from_sat_per_kwu(f.to_sat() / 4);
            }
        }
        let diff: f64 = fee_rate.to_sat_per_kwu().abs_diff(psbt_fee_rate.to_sat_per_kwu()) as f64;
        // convert config field to percentage
        let acceptable_fee_rate_diff: f64 = (self.config.fee_rate_diff_percentage as f64) / 100.0;
        if diff > acceptable_fee_rate_diff * (fee_rate.to_sat_per_kwu() as f64) {
            return Err(SigningRound1Error::FeeRateDifferenceTooGreat);
        }

        let tx = psbt.clone().extract_tx();
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

        let key_package =
            self.db.get_key_package()?.ok_or(SigningRound1Error::MissingKeyPackage)?;
        // Get our secret package
        let secret = key_package.signing_share();
        let mut nonces = vec![];

        let mut rng = thread_rng();
        // Order here is important for both the signer and cordinator
        // Each nonce pair is commitment to a input of the tx
        // When the signing package is produced the signer should be careful to
        // Verify that the nonce pairs are in the same order as the inputs
        for _ in 0..num_inputs {
            let nonce_pkg = frost::round1::commit(secret, &mut rng);
            nonces.push(nonce_pkg);
        }

        let signing_commitments =
            nonces.iter().map(|nonce| nonce.1).collect::<Vec<frost::round1::SigningCommitments>>();
        // Add the signing commitments to the psbt
        add_signing_commitments_to_psbt(&mut psbt, &signing_commitments, &self.identifier);

        // Save signing nonces in memory
        let signing_nonces =
            nonces.iter().map(|nonce| (nonce.0.clone(), nonce.1.clone())).collect::<Vec<_>>();

        nonces_lock.replace(signing_nonces.clone());
        Ok(())
    }

    pub(crate) async fn get_round2_signing_package(
        &self,
        mut psbt: &mut Psbt,
    ) -> Result<(), SigningRound2Error> {
        // Important note here is that we never re-use the same nonce pairs for a different signing
        // request Should always generate new ones or if we are in a signing session refuse
        // to provide new ones
        let key_package =
            self.db.get_key_package()?.ok_or(SigningRound2Error::MissingKeyPackage)?;
        let tx = psbt.clone().extract_tx();
        let num_inputs = tx.input.len();
        let mut signing_packages = psbt_to_signing_packages(psbt)?;

        // Get signing nonces from round 1
        let mut nonces_lock = self.frost_round1_nonces.lock().await;
        let signing_nonces =
            nonces_lock.clone().ok_or(SigningRound2Error::MissingRound1SigningNonce)?;

        if signing_nonces.len() != num_inputs {
            return Err(SigningRound2Error::InvalidSigningPackage(
                "Number of signing nonces does not match number of inputs",
            ));
        }
        let tx = psbt.clone().extract_tx();
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

        // Get a parital sig for each input
        let mut partial_sigs = vec![];
        for (index, (signing_package, _txin)) in
            signing_packages.iter_mut().zip(tx.input.iter()).enumerate()
        {
            partial_sigs.push(frost::round2::sign(
                &signing_package,
                &signing_nonces.get(index).expect("valid index").0,
                &key_package,
            )?);
        }
        // Add partial sig to psbt
        add_partial_signature_to_psbt(&mut psbt, &partial_sigs, &self.identifier);

        // Clear the signing nonces
        // This finalizes the signing session
        nonces_lock.take();
        Ok(())
    }

    pub(crate) fn finalize_signer(
        &self,
        outputs: Vec<TxOut>,
        fee_rate: FeeRate,
        witness: Vec<Vec<u8>>,
    ) -> Result<Psbt, SigningFinalizeError> {
        let key_package =
            self.db.get_key_package()?.ok_or(SigningFinalizeError::MissingKeyPackage)?;
        let secp_pk = key_package.verifying_key().to_secp_pk().expect("valid pk");
        let change_script =
            reth_btc_wallet::address::generate_taproot_change_scriptpubkey(&SECP, &secp_pk);
        let mut original_psbt = self.make_tx(outputs, fee_rate, change_script).unwrap();

        let hash_ty = bitcoin::sighash::TapSighashType::All;
        let sighash_type = bitcoin::psbt::PsbtSighashType::from(hash_ty);
        // Add witness to the psbt
        for (index, w) in witness.iter().enumerate() {
            let signature = bitcoin::taproot::Signature::from_slice(w.as_slice())?;
            original_psbt.inputs[index].sighash_type = Some(sighash_type);
            original_psbt.inputs[index].tap_key_sig = Some(signature);
        }

        if let Err(errs) = original_psbt.finalize_mut(&SECP) {
            error!("Signer finalize: Had {} PSBT finalization errors:", errs.len());
            for e in &errs {
                error!("  PSBT finalization error: {}", e);
            }
            return Err(SigningFinalizeError::PsbtFinalizationFailed(errs));
        }

        let (change_outputs, selected_inputs) =
            util::add_remove_utxo_from_psbt(&original_psbt, &secp_pk);
        self.db.add_remove_utxos(selected_inputs.into_iter(), change_outputs.into_iter())?;
        self.db.flush()?;

        Ok(original_psbt)
    }
}
