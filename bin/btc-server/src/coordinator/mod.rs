use log::{debug, error, info};

use crate::{
    coordinator::error::CoordinatorError,
    database::{Db, Error as DbError, Utxo},
    pegout_id::PegoutId,
    pegout_scheduler::Tx,
    telemetry::Telemetry,
    util::{validate_psbt, NO_FLAGS, ROUND1, ROUND1_TRANSITION, ROUND2},
    wallet::{
        coin_selection,
        psbt::{PsbtExt as BtcPsbtExt, PsbtInputExt},
    },
};
use bitcoin::{psbt::Psbt, FeeRate, OutPoint, ScriptBuf, TxOut};
use frost_secp256k1_tr::{self as frost, keys::Tweak, SigningParameters};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

pub mod error;

#[allow(dead_code)]
const MIN_RELAY_FEE_RATE_SAT_VB: u64 = 1;

pub fn add_round1_signing(
    signing_session_id: &[u8; 32],
    frost_id: frost::Identifier,
    psbt: &Psbt,
    db: &Db,
    min_signers: u16,
) -> Result<(), CoordinatorError> {
    let _start = Instant::now();
    validate_psbt(psbt, ROUND1, min_signers, db)?;

    info!("psbt() = {}", psbt);

    for input in &psbt.inputs {
        let sc = input.signing_commitments(frost_id);
        info!("Adding signing commitment for frost id: {:?}", frost_id);
        info!("sc.keys() = {:?}", sc);

        if sc.is_none() {
            return Err(CoordinatorError::CouldNotFindParticipantInformation());
        }
    }

    // TODO Need to check this psbt affect the other inputs and outputs
    // Note: There doesn't need to be a check for a quorum of round1 signing packages
    // The more the better in the case one is unresponsive
    // the frost lib will check if we have enough when we create the signing package
    db.update_psbt(signing_session_id, psbt)?;
    db.flush()?;
    debug!("Stored round1 signing from peer: {:?}", frost_id);

    Ok(())
}

pub fn add_round2_signing(
    signing_session_id: &[u8; 32],
    frost_id: frost::Identifier,
    psbt: &Psbt,
    db: &Db,
    min_signers: u16,
) -> Result<(), CoordinatorError> {
    // validate PSBT
    validate_psbt(psbt, ROUND2, min_signers, db)?;

    db.update_psbt(signing_session_id, psbt)?;
    db.flush()?;
    debug!("Stored round2 signing from peer: {:?}", frost_id);

    Ok(())
}

pub fn make_tx(
    outputs: Vec<(TxOut, PegoutId)>,
    fee_rate: FeeRate,
    change_script: ScriptBuf,
    db: &Db,
    min_signers: u16,
    tracked_txs: Vec<Tx>,
    telemetry: Option<Arc<Telemetry>>,
    btc_network: bitcoin::network::Network,
    identifier: u16,
) -> Result<Psbt, CoordinatorError> {
    // TODO: re-enable this check
    // Ensure we are above the minimum relay fee rate
    // let mut fee_rate = fee_rate;
    // let mut fee_rate = FeeRate::from_sat_per_vb_unchecked()
    // let min_relay_fee_rate = FeeRate::from_sat_per_vb_unchecked(MIN_RELAY_FEE_RATE_SAT_VB);
    // if fee_rate < min_relay_fee_rate {
    //     fee_rate = min_relay_fee_rate;
    // }

    // collect all database utxos in a hashmap
    let utxos: HashMap<OutPoint, Utxo> =
        db.iter_utxos().try_fold(HashMap::new(), |mut map, r| {
            let utxo = r?; // Directly propagate the error with `?`
            map.insert(utxo.outpoint, utxo);
            Ok::<HashMap<bitcoin::OutPoint, Utxo>, DbError>(map)
        })?;
    debug!("utxos len = {:?}", utxos.len());
    debug!("utxos = {:?}", utxos);
    if let Some(telemetry) = telemetry {
        telemetry.update_utxos(btc_network, identifier, utxos.len() as i64);
    }

    let tracked_inputs = tracked_txs
        .iter()
        .flat_map(|tx| tx.inputs().collect::<Vec<OutPoint>>())
        .collect::<HashSet<OutPoint>>();
    debug!("tracked_inputs len = {:?}", tracked_inputs.len());
    debug!("tracked_inputs = {:?}", tracked_inputs);

    // Filter utxos that are still pending and conflict with pending txs.
    let mut available_utxos = utxos
        .clone()
        .into_iter()
        .filter(|(p, _u)| !tracked_inputs.contains(p))
        .collect::<HashMap<_, _>>();
    debug!("available_utxos len = {:?}", available_utxos.len());
    debug!("available_utxos = {:?}", available_utxos);

    // if we are retrying pegouts, we need to add a conflicting input for each tracked tx
    // that honors each pegout
    let tracked_pegout_request_ids = tracked_txs
        .iter()
        .flat_map(|tx| tx.pegout_requests.iter().map(|p| p.id))
        .collect::<HashSet<_>>();
    debug!("tracked_pegout_request_ids = {:?}", tracked_pegout_request_ids);

    // Collect all pegout ids being retried.
    let matching_pegouts_ids: Vec<&PegoutId> = outputs
        .iter()
        .filter(|(_, pegout_id)| tracked_pegout_request_ids.contains(pegout_id))
        .map(|(_, pegout_id)| pegout_id)
        .collect();
    debug!("matching_pegouts_ids = {:?}", matching_pegouts_ids);

    // get a tracked input for each matching pegout
    let matching_tracked_inputs: Result<Vec<OutPoint>, CoordinatorError> = tracked_txs
        .iter()
        .filter(|tx| tx.pegout_requests.iter().any(|p| matching_pegouts_ids.contains(&&p.id)))
        .map(|tx| tx.inputs().next().ok_or_else(|| CoordinatorError::NoConflictingInputs))
        .collect();
    let matching_tracked_inputs = matching_tracked_inputs?;
    debug!("matching_tracked_inputs = {:?}", matching_tracked_inputs);

    // get the utxo for each matching tracked input
    let mut conflicting_utxos: HashMap<OutPoint, Utxo> = HashMap::new();
    let conflicting_inputs: Result<Vec<Utxo>, CoordinatorError> = matching_tracked_inputs
        .iter()
        .map(|op| {
            utxos.get(op).ok_or_else(|| CoordinatorError::MissingUtxoForConflictingInput).map(
                |u: &Utxo| {
                    // Conflicting utxos will be added to available utxos before finishing
                    // coin selection
                    conflicting_utxos.insert(*op, u.clone());
                    u.clone()
                },
            )
        })
        .collect();

    let _ = conflicting_inputs?;
    debug!("conflicting_utxos = {:?}", conflicting_utxos);

    // include conflicting utxos when selecting from available utxos
    conflicting_utxos.iter().for_each(|(op, u)| {
        available_utxos.insert(*op, u.clone());
    });

    let psbt = coin_selection::coin_selection(
        available_utxos,
        conflicting_utxos,
        outputs,
        fee_rate,
        change_script,
    )?;

    // Sanity check that we created a valid PSBT
    // This should not fail
    validate_psbt(&psbt, NO_FLAGS, min_signers, db)?;

    Ok(psbt)
}

/// If no Err is return the original psbt served to this function is good to go out to the
/// signers nothing needs to be added to it as the signers all provided their signing
/// commitments already and the coordinator just need to verify them
pub fn get_to_sign(
    signing_session_id: &[u8; 32],
    db: &Db,
    min_signers: u16,
) -> Result<Psbt, CoordinatorError> {
    // Note that the tweaks and signing commitments should be explicitly verified by the signers
    // before signing Instead we can add it to the psbt as a proprietary field for each
    // input Lastly save this to sign package to the db

    if let Some(psbt) = db.get_psbt(signing_session_id)? {
        for input in &psbt.inputs {
            let sc = input.all_signing_commitments();
            info!("sc.len() = {}", sc.len());
            if sc.len() < min_signers as usize {
                return Err(CoordinatorError::NotEnoughSigners);
            }
        }

        validate_psbt(&psbt, ROUND1_TRANSITION, min_signers, db)?;
        return Ok(psbt);
    }

    Err(CoordinatorError::CouldNotFindPsbt)
}

/// Returns finalized and ready to broadcast tx
pub async fn finalize_signing(
    signing_session_id: &[u8; 32],
    db: &Db,
) -> Result<Psbt, CoordinatorError> {
    // Lock here to prevent a make_tx that uses utxos that will be removed
    let mut psbt = db.get_psbt(signing_session_id)?.ok_or(CoordinatorError::CouldNotFindPsbt)?;

    let pk_package = db.get_public_key_package()?.ok_or(CoordinatorError::MissingKeyPackage)?;
    // Get signing packages for this signing session
    let signing_packages =
        psbt.signing_packages().map_err(CoordinatorError::PsbtToSigningPackageConversionError)?;

    for (index, psbt_input) in psbt.inputs.iter_mut().enumerate() {
        let signing_package = signing_packages
            .get(index)
            .ok_or(CoordinatorError::MissingSigningPackageAtIndex(index))?;
        let partial_sig = psbt_input.all_partial_signatures();
        let eth_address_tweak = psbt_input.eth_address();
        let signing_parameters = SigningParameters {
            tapscript_merkle_root: None,
            additional_tweak: eth_address_tweak.map(|e| e.to_vec()),
        };
        let agg_sig = frost::aggregate_with_tweak(
            signing_package,
            &partial_sig,
            &pk_package,
            &signing_parameters,
        )?;

        let effective_key = pk_package.clone().tweak(&signing_parameters);
        // Verify signature -- redundant check finalize psbt already checks this
        effective_key.verifying_key().verify(signing_package.message(), &agg_sig)?;

        let secp_sig = bitcoin::secp256k1::schnorr::Signature::from_slice(&agg_sig.serialize()?)?;

        // Note: we don't need to add the internal key here for a key spend path
        // as the output key is derived from the scriptpubkey
        let hash_ty = bitcoin::sighash::TapSighashType::All;
        let sighash_type = bitcoin::psbt::PsbtSighashType::from(hash_ty);
        psbt_input.sighash_type = Some(sighash_type);
        psbt_input.tap_key_sig =
            Some(bitcoin::taproot::Signature { signature: secp_sig, sighash_type: hash_ty });
    }

    // Keep a copy of the original psbt as we need to add back the signing commitments and
    // partial signatures `finalize_mut` removes everything that is not a witness to the
    // inputs

    // TODO: secp context should be a global variable or passed down
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let mut original_psbt = psbt.clone();
    if let Err(errs) = miniscript::psbt::PsbtExt::finalize_mut(&mut psbt, &secp) {
        error!("Had {} PSBT finalization errors:", errs.len());
        for e in &errs {
            error!("PSBT finalization error: {}", e);
        }
        return Err(CoordinatorError::PbstFinalizationFailed(errs));
    }

    for (index, input) in original_psbt.inputs.iter_mut().enumerate() {
        input.final_script_witness = Some(
            psbt.inputs[index]
                .final_script_witness
                .clone()
                .ok_or(CoordinatorError::MissingFinalScript)?,
        );
    }

    Ok(original_psbt)
}
