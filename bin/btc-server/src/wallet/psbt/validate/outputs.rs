use crate::database::Error;
use crate::pegout_id::PegoutId;
use crate::wallet::psbt::validate::data_provider::{
    PendingPegoutData, PsbtDataProvider, PsbtDataProviderError,
};
use crate::wallet::psbt::validate::error::PsbtValidationError;
use crate::wallet::psbt::{PsbtExt, PsbtOutputExt};
use bitcoin::{Amount, Psbt, ScriptBuf};

fn validate_outputs(
    psbt: &Psbt,
    data_provider: impl PsbtDataProvider,
) -> Result<Option<PsbtValidationError>, PsbtDataProviderError> {
    // PSBT must have at least one output
    if psbt.outputs.is_empty() {
        return Ok(Some(PsbtValidationError::NoOutputs));
    }

    // Ensure psbt.outputs and psbt.unsigned_tx.output have the same number of elements.
    // This is critical to prevent a malicious coordinator from adding arbitrary outputs
    // to psbt.unsigned_tx.output that are not declared in psbt.outputs.
    if psbt.outputs.len() != psbt.unsigned_tx.output.len() {
        return Ok(Some(PsbtValidationError::OutputCountMismatch {
            psbt_outputs_count: psbt.outputs.len(),
            unsigned_tx_outputs_count: psbt.unsigned_tx.output.len(),
        }));
    };

    // Let's do quick (no IO) validations first
    let mut seen_pegout_ids = std::collections::HashSet::new();
    let mut change_output_index: Option<usize> = None;
    let mut enumerated_pegout_outputs = Vec::with_capacity(psbt.outputs.len() + 1);

    for (index, output) in psbt.outputs.iter().enumerate() {
        let pegout_id = match output.pegout_id_bytes() {
            // This is pegout output
            Ok(Some(bytes)) => PegoutId::from(bytes),
            // This is a change output
            Ok(None) => {
                // PSBT must have not more than one change output
                if let Some(existing_chain_output_index) = change_output_index {
                    return Ok(Some(PsbtValidationError::ExpectingOnlyOneChangeOutput {
                        first_output: psbt.outputs[existing_chain_output_index].clone(),
                        second_output: output.clone(),
                    }));
                }

                change_output_index = Some(index);

                continue;
            }
            // Invalid pegout output
            Err(e) => {
                return Ok(Some(PsbtValidationError::InvalidPegoutIdBytes {
                    error: e,
                    output: output.clone(),
                }));
            }
        };

        // We must not have duplicate pegout IDs
        if !seen_pegout_ids.insert(pegout_id) {
            return Ok(Some(PsbtValidationError::DuplicatePegoutId(pegout_id)));
        }

        enumerated_pegout_outputs.push((index, pegout_id));
    }

    // We must have at least one pegout output
    if enumerated_pegout_outputs.is_empty() {
        return Ok(Some(PsbtValidationError::NoPegoutOutputs));
    }

    // We must have a change output
    if change_output_index.is_none() {
        return Ok(Some(PsbtValidationError::NoChangeOutput));
    }

    let fee_per_output = match psbt.fee_per_output(seen_pegout_ids.len() as u64) {
        Ok(fee) => fee,
        Err(error) => {
            return Ok(Some(PsbtValidationError::FeePerOutputCalculationFailed(error)));
        }
    };

    // Heavy (potential IO) validation
    for (index, pegout_id) in enumerated_pegout_outputs {
        // Pegout must not be finalized
        if data_provider.contains_finalized_pegout(&pegout_id)? {
            return Ok(Some(PsbtValidationError::AlreadyFinalizedPegout(pegout_id)));
        }

        // Pending pegout data must be present
        let Some(PendingPegoutData { amount, script_pubkey }) =
            data_provider.pending_pegout_data(&pegout_id)?
        else {
            return Ok(Some(PsbtValidationError::PendingPegoutDataNotFound(pegout_id)));
        };

        // Retrieve the corresponding TxOut from the PSBT, according to the
        // specified pegout index.
        let Some(tx_out) = psbt.unsigned_tx.output.get(index) else {
            return Ok(Some(PsbtValidationError::UnsignedTxOutputNotFound { index, pegout_id }));
        };

        // check if a corresponding output exists in the psbt and is for the right amount
        validate_psbt_by_output(tx_out, script_pubkey, amount, fee_per_output)?;
    }

    // Verify that there is at most one change output
    // and that all outputs are either a validated pegout or the single change output.

    // The preceding checks (output length equality, duplicate pegout ID, and change output count)
    // ensure that every entry in `psbt.unsigned_tx.output` (due to the length check)
    // is accounted for as either a pegout (validated in the main loop below)
    // or the single allowed change output. Any other scenario (e.g., undeclared outputs,
    // too many change outputs) would have triggered an earlier error.

    // TODO: is it fine not having a change output?
    // if a change output exists, check if it is valid
    if let Some(idx) = change_output_index {
        // TODO: No panic
        let agg_pk = aggregate_public_key.expect("we should have it");

        let expected_script_pubkey = generate_taproot_change_scriptpubkey(&agg_pk);

        let change_output =
            psbt.unsigned_tx.output.get(idx).ok_or(ValidateOutputsError::InvalidChangeOutput)?;

        let has_correct_change = change_output.script_pubkey == expected_script_pubkey;
        if !has_correct_change {
            return Err(ValidateOutputsError::InvalidChangeOutput);
        }
    }

    Ok(())
}

/// Validates a transaction output against an expected destination address and
/// amount.
///
/// This function verifies that a transaction output pays to the correct
/// destination and contains the correct amount after subtracting the fee.
pub fn validate_psbt_by_output(
    tx_out: &bitcoin::TxOut,
    script_pubkey: ScriptBuf,
    amount: Amount,
    fee_per_output: Amount,
) -> Result<(), PsbtValidationError> {
    if tx_out.script_pubkey != script_pubkey {
        tracing_actix_web::root_span_macro::private::tracing::error!(target: "consensus::authority::frost_task::validate_psbt_by_ids", "Output script pubkey does not match destination");
        return Err(PsbtValidationError::FailedToValidatePsbtByIds(String::from(
            "Output script pubkey does not match destination",
        )));
    }

    let Some(expected_amount) = amount.checked_sub(fee_per_output) else {
        return Err(PsbtValidationError::FailedToValidatePsbtByIds(String::from(
            "Calculating expected amount caused an underflow",
        )));
    };

    if tx_out.value != expected_amount {
        return Err(PsbtValidationError::FailedToValidatePsbtByIds(String::from(
            "The output value does not match the expected amount",
        )));
    }

    Ok(())
}
