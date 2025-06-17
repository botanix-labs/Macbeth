use crate::{
    util::ROUND2,
    wallet::psbt::{PsbtExt, PsbtInputExt},
};
use bitcoin::psbt::Psbt;
use error::{SigningRound1Error, SigningRound2Error};
use frost_secp256k1_tr::{
    self as frost,
    round1::{SigningCommitments, SigningNonces},
    Identifier, SigningParameters,
};
use rand::thread_rng;

use crate::{
    database::Db as Database,
    util::{validate_psbt, ROUND1},
};

pub mod error;

pub fn get_round1_signing_package(
    psbt: &mut Psbt,
    min_signers: u16,
    db: &Database,
    my_identifier: &Identifier,
) -> Result<Vec<(SigningNonces, SigningCommitments)>, SigningRound1Error> {
    // TODO: re-enable this check
    // check fee is within acceptable range
    // let psbt_fee_rate =
    //     psbt.fee_rate().ok_or(SigningRound1Error::FailedToGetFeeRateFromPsbt)?;
    // // fetch fee rate from bitcoind
    // let fee_res = self.bitcoind_client.estimate_smart_fee(1,
    // Some(EstimateMode::Conservative));

    // let mut fee_rate = self.fall_back_fee_rate;
    // if let Ok(fee) = fee_res {
    //     if let Some(f) = fee.fee_rate {
    //         fee_rate = util::btc_per_kb_to_sat_per_vb(fee_rate)
    //     }
    // }
    // let diff = fee_rate.to_sat_per_kwu().abs_diff(psbt_fee_rate.to_sat_per_kwu()) as f64;
    // // convert config field to percentage
    // let acceptable_fee_rate_diff = ((self.config.fee_rate_diff_percentage as f64) / 100.0) *
    //     fee_rate.to_sat_per_kwu() as f64;

    // TODO: re-enable this check
    // if diff > acceptable_fee_rate_diff {
    //     debug!("[signer] fee rate difference is too great");
    //     debug!("[signer] acceptable fee rate difference: {:?}", acceptable_fee_rate_diff);
    //     debug!("[signer] fee rate difference: {:?}", diff);
    //     debug!("[signer] fee rate from bitcoind/fallback: {:?}", fee_rate);
    //     debug!("[signer] fee rate from psbt: {:?}", psbt_fee_rate);
    //     debug!(
    //         "[signer] config fee rate diff percentage: {:?}",
    //         self.config.fee_rate_diff_percentage
    //     );
    //     return Err(SigningRound1Error::FeeRateDifferenceTooGreat);
    // }

    // Validate PSBT
    validate_psbt(psbt, ROUND1, min_signers, db)?;

    let num_inputs = psbt.inputs.len();

    let key_package = db.get_key_package()?.ok_or(SigningRound1Error::MissingKeyPackage)?;
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
        psbt.inputs[i].set_signing_commitment(*my_identifier, &nonce_pkg.1);
        nonces.push(nonce_pkg);
    }

    Ok(nonces)
}

/// Important note here is that we never reuse the same nonce pairs for a different signing
/// request Should always generate new ones or if we are in a signing session refuse
/// to provide new ones
pub fn get_round2_signing_package(
    psbt: &mut Psbt,
    min_signers: u16,
    db: &Database,
    identifier: &Identifier,
    // Each nonce pair is commitment to a input of the tx
    signing_nonces: &[(SigningNonces, SigningCommitments)],
) -> Result<(), SigningRound2Error> {
    validate_psbt(psbt, ROUND2, min_signers, db)?;

    let tx = psbt.clone().extract_tx()?;
    let num_inputs = tx.input.len();
    let mut signing_packages = psbt.signing_packages()?;

    if signing_nonces.len() != num_inputs {
        let err = SigningRound2Error::InvalidSigningPackage(
            "Number of signing nonces does not match number of inputs",
        );
        return Err(err);
    }
    for (index, signing_package) in signing_packages.iter().enumerate() {
        // Check if this signer is in the signing set
        // This should also implicitly validate the psbt
        // In other words this signer would have never provided nonce pairs if the psbt was not
        // valid from round 1
        let signing_commitments = signing_package.signing_commitments();
        if !signing_commitments.contains_key(identifier) {
            return Err(SigningRound2Error::SignerNotFound(index));
        }
        let our_sc = signing_commitments.get(identifier).expect("valid index");
        let our_nonce = signing_nonces.get(index).expect("valid index");
        if our_sc != &our_nonce.1 {
            let err =
                SigningRound2Error::InvalidSigningPackage("Invalid nonce pair for this signer");
            return Err(err);
        }
    }

    let key_package = db.get_key_package()?.ok_or(SigningRound2Error::MissingKeyPackage)?;

    // Get a partial signature for each input
    for (index, (signing_package, psbt_in)) in
        signing_packages.iter_mut().zip(psbt.inputs.iter_mut()).enumerate()
    {
        let eth_address_tweak = psbt_in.eth_address();
        // TODO this will need to be revisited when we add tapleaves as all signatures will need
        // to tweak with the merkel root
        let signing_parameters = SigningParameters {
            tapscript_merkle_root: None,
            additional_tweak: eth_address_tweak.map(|e| e.to_vec()),
        };
        let sig = frost::round2::sign_with_tweak(
            signing_package,
            &signing_nonces.get(index).expect("valid index").0,
            &key_package,
            &signing_parameters,
        )?;
        psbt_in.set_partial_signature(*identifier, &sig);
    }

    // perform sanity checks for fees
    let _tx = psbt.clone().extract_tx()?;

    Ok(())
}

// Currently not used
// pub(crate) async fn finalize_signer(
//     finalized_psbt: Psbt,
// ) -> Result<Psbt, SigningFinalizeError> {
//     let mut finalized_psbt = finalized_psbt.clone();
//     let _key_package =
// self.db.get_key_package()?.ok_or(SigningFinalizeError::MissingKeyPackage)?;     let pk_package =
//         self.db.get_public_key_package()?.ok_or(SigningFinalizeError::MissingKeyPackage)?;

//     let signing_packages = finalized_psbt
//         .signing_packages()
//         .map_err(SigningFinalizeError::PsbtToSigningPackageConversionError)?;

//     // Verify each inputs signature by aggregating and then verifying
//     for (index, psbt_input) in finalized_psbt.inputs.iter_mut().enumerate() {
//         let signing_package = signing_packages.get(index).expect("valid index").clone();
//         let partial_sig = psbt_input.all_partial_signatures();
//         let agg_sig = frost::aggregate(&signing_package, &partial_sig, &pk_package)?;

//         // Verify signature
//         if let Some(e) = psbt_input.eth_address() {
//             // TODO(armins) tapscript merkle root will need to be updated when we add tapleaves
//             let signing_parameters = SigningParameters {
//                 tapscript_merkle_root: None,
//                 additional_tweak: Some(e.clone().to_vec()),
//             };
//             let effective_key = pk_package.clone().tweak(&signing_parameters);
//             effective_key.verifying_key().verify(signing_package.message(), &agg_sig)?;
//         } else {
//             pk_package.verifying_key().verify(signing_package.message(), &agg_sig)?;
//         }
//     }

//     validate_outputs(&finalized_psbt, &self.db)?;

//     let tx = finalized_psbt.clone().extract_tx()?;

//     // Lets broadcast the tx
//     let tx_id = match self.bitcoind_client.send_raw_transaction(&tx) {
//         Ok(tx_id) => Ok(Some(tx_id)),
//         Err(err) => {
//             let err_msg = err.to_string();
//             if err_msg.contains("already in chain") {
//                 Ok(None)
//             } else {
//                 let err = CoordinatorError::FailedToBroadcastTx(err);
//                 if let Some(telemetry) = self.telemetry.as_ref() {
//                     telemetry.update_signing_error_metrics(
//                         self.btc_network,
//                         self.config.identifier,
//                         None,
//                         &err.to_string(),
//                     );
//                 }
//                 error!("Failed to broadcast tx: {}", err);
//                 Err(err)
//             }
//         }
//     }?;

//     if let Some(tx_id) = tx_id {
//         info!("Broadcasted tx: {:?}", tx_id);
//     } else {
//         info!("Transaction already broadcasted and in pool");
//     }

//     Ok(finalized_psbt)
// }
