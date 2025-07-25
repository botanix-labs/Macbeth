use crate::{
    database::version::UtxoVersion,
    wallet::{
        psbt::PegoutId as PegoutIdBytes,
        util::{calculate_signed_tx_weight, WalletCalculationError},
        TAPROOT_KEYSPEND_SATISFACTION_WEIGHT,
    },
};
use bdk_wallet::coin_selection::{
    CoinSelectionAlgorithm, InsufficientFunds, OldestFirstCoinSelection,
};
use bitcoin::{
    amount::CheckedSum,
    psbt::{Error as PsbtError, ExtractTxError, Psbt},
    Amount, FeeRate, OutPoint, ScriptBuf, TxOut,
};
use log::debug;
use std::{cmp::Reverse, collections::HashMap};
use thiserror::Error;

use crate::{database::Utxo, pegout_id::PegoutId, util::OutPointExt};

#[derive(Debug, Error)]
pub enum CoinSelectionError {
    #[error("Coin selection error: {0}")]
    CoinSelectionBdk(#[from] InsufficientFunds),
    #[error("PSBT error: {0}")]
    PsbtError(#[from] PsbtError),
    #[error("Extract tx error: {0}")]
    ExtractTxError(#[from] ExtractTxError),
    #[error("Outputs cannot be empty")]
    OutputsCannotBeEmpty,
    #[error("Pegout value is smaller than pegout fee")]
    PegoutFeeOverflow,
    #[error("Fee rate overflow")]
    FeeRateOverflow,
    #[error("Total utxos value overflow")]
    TotalUtxosValueOverflow,
    #[error("Sanity check error - SHOULD NOT HAPPEN: {0}")]
    SanityCheckError(#[from] SanityCheckError),
    #[error("No viable outputs after applying fees and filtering dust")]
    NoViableOutputs,
    #[error("Wallet calculation error: {0}")]
    WalletCalculationError(#[from] WalletCalculationError),
}

#[derive(Debug, Error)]
pub enum SanityCheckError {
    #[error("Bad refund balance in change output")]
    /// Failed validation: `change_output_value == total_input_value - target_amount`
    BadRefundBalance {
        change_output_value: Amount,
        total_input_value: Amount,
        target_amount: Amount,
    },
    #[error("Recalculation filtered out more outputs than the original attempt")]
    RecalculationFilteredMoreOutputs,
}

/// Result of applying fees and filtering dust outputs
#[derive(Debug, Clone)]
pub enum FilterResult {
    /// All outputs were retained after applying fees
    AllRemaining(Vec<(TxOut, PegoutIdBytes)>),
    /// Some outputs were filtered out due to dust or insufficient funds
    SomeFiltered {
        /// The pegout ids that were filtered out
        filtered_pegout_ids: Vec<PegoutIdBytes>,
    },
}

impl PartialEq for CoinSelectionError {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

// Change calculation constants
const TARGET_CHANGE_PERCENT: u64 = 50; // 50% of total pegout value
const MAX_CHANGE_PERCENT: u64 = 5; // 5% of total UTXOs
const MIN_CHANGE_SATS: u64 = 10_000; // minimum pegout value (0.0001 BTC)

/// Coin selection
pub(crate) fn coin_selection(
    optional_utxos: HashMap<OutPoint, Utxo>,
    required_utxos: HashMap<OutPoint, Utxo>,
    outputs: Vec<(TxOut, PegoutId)>,
    fee_rate: FeeRate,
    change_script: ScriptBuf,
) -> Result<Psbt, CoinSelectionError> {
    // Input validation
    if outputs.is_empty() {
        return Err(CoinSelectionError::OutputsCannotBeEmpty);
    }

    // Basic calculations
    let pegouts = outputs
        .into_iter()
        .map(|(txout, pegout_id)| (txout, pegout_id.as_bytes()))
        .collect::<Vec<_>>();
    let required_utxos_value = required_utxos
        .values()
        .map(|u| u.output.value)
        .checked_sum()
        .ok_or(CoinSelectionError::TotalUtxosValueOverflow)?;
    let optional_utxos_value = optional_utxos
        .values()
        .map(|u| u.output.value)
        .checked_sum()
        .ok_or(CoinSelectionError::TotalUtxosValueOverflow)?;
    let total_utxos_value = required_utxos_value
        .checked_add(optional_utxos_value)
        .ok_or(CoinSelectionError::TotalUtxosValueOverflow)?;
    let total_pegout_target = pegouts
        .iter()
        .map(|(txout, _)| txout.value)
        .checked_sum()
        .ok_or(CoinSelectionError::TotalUtxosValueOverflow)?;

    // return InsufficientFunds error
    let remaining_utxos_value = total_utxos_value
        .checked_sub(total_pegout_target)
        .ok_or(InsufficientFunds { needed: total_pegout_target, available: total_utxos_value })?;

    // Coin selection using BDK
    let target_change = calculate_target_change(total_pegout_target, remaining_utxos_value)?;
    let coin_selection_target = total_pegout_target
        .checked_add(target_change)
        .ok_or(CoinSelectionError::FeeRateOverflow)?;
    let coin_selection_algorithm =
        bdk_wallet::coin_selection::BranchAndBoundCoinSelection::new(0, OldestFirstCoinSelection);
    let selected_inputs = perform_coin_selection(
        coin_selection_algorithm,
        optional_utxos,
        required_utxos,
        fee_rate,
        coin_selection_target,
        &change_script,
    )?;

    let (psbt, filtered_pegout_ids) = apply_fees_and_create_psbt(
        &selected_inputs,
        pegouts.clone(),
        change_script.clone(),
        fee_rate,
    )?;

    // update total pegout target to reflect the filtered pegout ids
    let updated_pegout_target =
        pegouts
            .iter()
            .filter_map(|(txout, pegout_id)| {
                if filtered_pegout_ids.contains(pegout_id) {
                    None
                } else {
                    Some(txout.value)
                }
            })
            .sum::<Amount>();

    sanity_check_psbt(&psbt, &selected_inputs, change_script.clone(), updated_pegout_target)?;

    Ok(psbt)
}

fn perform_coin_selection<T: CoinSelectionAlgorithm>(
    coin_selection_algorithm: T,
    optional_utxos: HashMap<OutPoint, Utxo>,
    required_utxos: HashMap<OutPoint, Utxo>,
    fee_rate: FeeRate,
    coin_selection_target: Amount,
    change_script: &ScriptBuf,
) -> Result<Vec<crate::wallet::psbt::InputDTO>, CoinSelectionError> {
    let mut rng = rand::thread_rng();

    let selection = coin_selection_algorithm
        .coin_select(
            required_utxos.values().map(utxo_to_bdk).collect::<Vec<_>>(),
            optional_utxos.values().map(utxo_to_bdk).collect::<Vec<_>>(),
            fee_rate,
            coin_selection_target,
            &change_script, // drain_script
            &mut rng,
        )
        .map_err(CoinSelectionError::CoinSelectionBdk)?;

    // Convert selected UTXOs to input DTOs
    let selected = selection
        .selected
        .iter()
        .map(|s| optional_utxos.get(&OutPoint::from_bdk(s.outpoint())))
        .filter_map(|s| if s.is_some() { s } else { None })
        .collect::<Vec<_>>();

    let selected_inputs: Vec<crate::wallet::psbt::InputDTO> = selected
        .iter()
        .map(|s| crate::wallet::psbt::InputDTO {
            outpoint: s.outpoint,
            output: s.output.clone(),
            eth_address: s.eth_address,
            version: UtxoVersion::try_from(s.version).ok().unwrap_or_default(),
        })
        .collect();

    Ok(selected_inputs)
}

fn apply_fees_and_create_psbt(
    selected_inputs: &[crate::wallet::psbt::InputDTO],
    pegouts: Vec<(TxOut, PegoutIdBytes)>,
    change_script: ScriptBuf,
    fee_rate: FeeRate,
) -> Result<(Psbt, Vec<PegoutIdBytes>), CoinSelectionError> {
    let change = create_change(&selected_inputs, &pegouts, change_script.clone())?;
    let absolute_fee = calculate_required_fee(&selected_inputs, &pegouts, &change, fee_rate)?;
    let first_attempt = try_apply_fees_and_filter_dust(pegouts.clone(), absolute_fee)?;

    let filtered_pegout_ids = Vec::new();
    match first_attempt {
        FilterResult::AllRemaining(final_pegouts) => {
            // No outputs were filtered out, we can return the psbt
            Ok((
                crate::wallet::psbt::create_psbt(selected_inputs.to_vec(), final_pegouts, change),
                filtered_pegout_ids,
            ))
        }
        FilterResult::SomeFiltered { filtered_pegout_ids } => {
            debug!(
                "Filtered out {} outputs due to dust or insufficient funds",
                filtered_pegout_ids.len()
            );

            let remaining_pegouts = pegouts
                .iter()
                .filter(|(_, pegout_id)| !filtered_pegout_ids.contains(pegout_id))
                .map(|(txout, pegout_id)| (txout.clone(), pegout_id.clone()))
                .collect::<Vec<_>>();

            // since a pegout was filtered out, this affects the total pegout value and therefore
            // the change output value
            let recalculated_change =
                create_change(&selected_inputs, &remaining_pegouts, change_script.clone())?;

            let recalculated_absolute_fee = calculate_required_fee(
                &selected_inputs,
                &remaining_pegouts,
                &recalculated_change,
                fee_rate,
            )?;

            let second_attempt =
                try_apply_fees_and_filter_dust(remaining_pegouts, recalculated_absolute_fee)?;

            match second_attempt {
                FilterResult::AllRemaining(final_outputs) => Ok((
                    crate::wallet::psbt::create_psbt(
                        selected_inputs.to_vec(),
                        final_outputs,
                        recalculated_change,
                    ),
                    filtered_pegout_ids,
                )),
                FilterResult::SomeFiltered { filtered_pegout_ids: _ } => {
                    // should never happen
                    return Err(SanityCheckError::RecalculationFilteredMoreOutputs.into());
                }
            }
        }
    }
}

fn try_apply_fees_and_filter_dust(
    pegouts: Vec<(TxOut, PegoutIdBytes)>,
    absolute_fee: Amount,
) -> Result<FilterResult, CoinSelectionError> {
    if pegouts.is_empty() {
        return Err(CoinSelectionError::NoViableOutputs);
    }

    let original_count = pegouts.len();
    let fees_to_subtract = calculate_fee_distribution(&pegouts, absolute_fee)?;

    let mut result = Vec::new();
    let mut filtered_pegout_ids = Vec::new();
    for (i, (txout, pegout_id)) in pegouts.into_iter().enumerate() {
        let script_pubkey = txout.script_pubkey.clone();
        let original_value = txout.value;

        let value_after_fee = txout.value.checked_sub(fees_to_subtract[i]).unwrap_or(Amount::ZERO);
        let updated_output = TxOut { value: value_after_fee, ..txout };

        let dust_threshold = updated_output.script_pubkey.minimal_non_dust();
        if updated_output.value >= dust_threshold {
            result.push((updated_output, pegout_id));
        } else {
            debug!("Filtered out pegout output due to dust: output_script={:?}, output_value={:?}, fee_to_subtract={:?}", 
            script_pubkey, original_value, fees_to_subtract[i]);
            filtered_pegout_ids.push(pegout_id);
        }
    }

    if result.len() == original_count {
        Ok(FilterResult::AllRemaining(result))
    } else if result.is_empty() {
        Err(CoinSelectionError::NoViableOutputs)
    } else {
        Ok(FilterResult::SomeFiltered { filtered_pegout_ids })
    }
}

/// Calculates the required absolute fee for a pegout tx with the given inputs, outputs, and fee
/// rate. This assumes the inputs are p2tr keyspend inputs (as is the case for all inputs in the
/// pegout tx).
fn calculate_required_fee(
    inputs: &[crate::wallet::psbt::InputDTO],
    outputs: &[(TxOut, PegoutIdBytes)],
    change: &Option<TxOut>,
    fee_rate: FeeRate,
) -> Result<Amount, CoinSelectionError> {
    let psbt = crate::wallet::psbt::create_psbt(inputs.to_vec(), outputs.to_vec(), change.clone());
    let total_weight = calculate_signed_tx_weight(&psbt)?;
    let absolute_fee = fee_rate.fee_wu(total_weight).ok_or(CoinSelectionError::FeeRateOverflow)?;

    Ok(absolute_fee)
}

/// Calculate the target change amount to balance competing goals:
///
/// **Benefits of larger change:**
/// - More useful for future pegouts (reduces need for multiple UTXOs)
/// - Provides UTXO consolidation over time
///
/// **Benefits of smaller change:**
/// - Keeps more liquidity available while the current pegout is waiting to be confirmed
///
/// The target change is calculated as a percentage of the pegout value, with a ceiling
/// to prevent excessive liquidity lockup and a floor to prevent the change from being too small.
fn calculate_target_change(
    total_pegout_value: Amount,
    remaining_utxos_value: Amount,
) -> Result<Amount, CoinSelectionError> {
    // default target change is a percentage of the total pegout value
    let target_change = total_pegout_value
        .checked_mul(TARGET_CHANGE_PERCENT)
        .ok_or(CoinSelectionError::FeeRateOverflow)?
        .checked_div(100)
        .ok_or(CoinSelectionError::FeeRateOverflow)?;

    let max_change_value = remaining_utxos_value
        .checked_mul(MAX_CHANGE_PERCENT)
        .ok_or(CoinSelectionError::FeeRateOverflow)?
        .checked_div(100)
        .ok_or(CoinSelectionError::FeeRateOverflow)?;

    Ok(target_change
        .min(max_change_value)
        // for small pegouts, set the change is at least the minimum change amount
        .max(Amount::from_sat(MIN_CHANGE_SATS))
        // this is an edge case, probably only relevant for test cases,
        // if the minimum is more than the remaining utxos just return the remaining utxos value
        .min(remaining_utxos_value))
}

fn calculate_fee_distribution(
    pegouts: &[(TxOut, PegoutIdBytes)],
    absolute_fee: Amount,
) -> Result<Vec<Amount>, CoinSelectionError> {
    let num_outputs = pegouts.len();
    if num_outputs == 0 {
        return Err(CoinSelectionError::OutputsCannotBeEmpty);
    }

    let base_fee_per_output =
        absolute_fee.checked_div(num_outputs as u64).ok_or(CoinSelectionError::FeeRateOverflow)?;
    let remainder = absolute_fee % num_outputs as u64;

    let mut fees_to_subtract = vec![base_fee_per_output; num_outputs];

    // Distribute the remainder one sat at a time to highest value outputs
    let mut sorted_indices: Vec<usize> = (0..num_outputs).collect();
    sorted_indices.sort_by_key(|&index| Reverse(pegouts[index].0.value));

    for sat_index in 0..remainder.to_sat() as usize {
        let index = sorted_indices[sat_index];
        fees_to_subtract[index] = fees_to_subtract[index]
            .checked_add(Amount::from_sat(1))
            .ok_or(CoinSelectionError::FeeRateOverflow)?;
    }
    Ok(fees_to_subtract)
}

fn create_change(
    selected_inputs: &[crate::wallet::psbt::InputDTO],
    pegouts: &[(TxOut, PegoutIdBytes)],
    change_script: ScriptBuf,
) -> Result<Option<TxOut>, CoinSelectionError> {
    let total_selected_inputs = selected_inputs.iter().map(|i| i.output.value).sum::<Amount>();
    let total_pegout_target = pegouts.iter().map(|(txout, _)| txout.value).sum::<Amount>();
    let final_change_amount = total_selected_inputs.checked_sub(total_pegout_target).ok_or(
        CoinSelectionError::CoinSelectionBdk(InsufficientFunds {
            needed: total_selected_inputs,
            available: total_pegout_target,
        }),
    )?;
    let change = Some(TxOut { script_pubkey: change_script.clone(), value: final_change_amount });
    Ok(change)
}

fn sanity_check_psbt(
    psbt: &Psbt,
    selected_inputs: &[crate::wallet::psbt::InputDTO],
    change_script: ScriptBuf,
    total_pegout_target: Amount,
) -> Result<(), CoinSelectionError> {
    let tx: bitcoin::Transaction = psbt.clone().extract_tx().unwrap();
    let total_input_value = selected_inputs.iter().map(|i| i.output.value).sum::<Amount>();
    let change_output_value =
        tx.output.iter().find(|o| o.script_pubkey == change_script).unwrap().value;

    // check that change output value = total_input_value - total_pegout_target
    // note that the fee comes out of the pegout target, not the change output
    if change_output_value !=
        total_input_value
            .checked_sub(total_pegout_target)
            .ok_or(CoinSelectionError::FeeRateOverflow)?
    {
        return Err(SanityCheckError::BadRefundBalance {
            change_output_value,
            total_input_value,
            target_amount: total_pegout_target,
        }
        .into());
    }

    Ok(())
}

/// Convert a UTXO to BDK's WeightedUtxo format
fn utxo_to_bdk(utxo: &Utxo) -> bdk_wallet::WeightedUtxo {
    bdk_wallet::WeightedUtxo {
        satisfaction_weight: TAPROOT_KEYSPEND_SATISFACTION_WEIGHT,
        utxo: bdk_wallet::Utxo::Local(bdk_wallet::LocalOutput {
            outpoint: utxo.outpoint.to_bdk(),
            txout: bdk_wallet::bitcoin::TxOut {
                script_pubkey: utxo.output.script_pubkey.to_bytes().into(),
                value: utxo.output.value,
            },
            keychain: bdk_wallet::KeychainKind::External,
            is_spent: false,
            derivation_index: 0,
            chain_position: bdk_wallet::chain::ChainPosition::Confirmed {
                anchor: bdk_wallet::chain::ConfirmationBlockTime::default(),
                transitively: None,
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::{
        database::Utxo,
        test_utils::{
            add_dummy_signatures_to_psbt, create_random_pegout_id, random_compute_txid,
            random_p2tr_keyspend_script, random_p2wpkh_script, random_p2wpkh_scriptpubkey,
        },
        wallet::{
            coin_selection::{calculate_target_change, CoinSelectionError, MIN_CHANGE_SATS},
            util::calculate_signed_tx_fee_rate,
        },
    };
    use bdk_wallet::{coin_selection::InsufficientFunds, psbt::PsbtUtils};
    use bitcoin::{Amount, FeeRate, OutPoint, Psbt, TapSighashType, TxOut};

    use super::coin_selection;

    #[test]
    fn test_calculate_target_change() {
        // Test default case: 50% of pegout value
        let total_pegout = Amount::from_sat(100_000);
        let remaining_utxos = Amount::from_sat(1_000_000);
        let target_change =
            calculate_target_change(total_pegout, remaining_utxos).expect("should not fail");
        assert_eq!(target_change, Amount::from_sat(50_000)); // 50% of 100k

        // Test max cap: 5% of remaining UTXOs
        let total_pegout = Amount::from_sat(20_000_000);
        let remaining_utxos = Amount::from_sat(100_000_000);
        let target_change =
            calculate_target_change(total_pegout, remaining_utxos).expect("should not fail");
        assert_eq!(target_change, Amount::from_sat(5_000_000)); // 5% of 100M, not 50% of 20M

        // Test min floor: at least 10,000 sats
        let total_pegout = Amount::from_sat(10_000);
        let remaining_utxos = Amount::from_sat(1_000_000);
        let target_change =
            calculate_target_change(total_pegout, remaining_utxos).expect("should not fail");
        assert_eq!(target_change, Amount::from_sat(MIN_CHANGE_SATS)); // Min of 10k, not 50% of 1k (500)

        // Test edge case: min > 5% of remaining UTXOs
        let total_pegout = Amount::from_sat(100_000);
        let remaining_utxos = Amount::from_sat(15_000);
        let target_change =
            calculate_target_change(total_pegout, remaining_utxos).expect("should not fail");
        assert_eq!(target_change, Amount::from_sat(MIN_CHANGE_SATS));

        // Test edge case: min > remaining UTXOs
        let total_pegout = Amount::from_sat(100_000);
        let remaining_utxos = Amount::from_sat(9_000);
        let target_change =
            calculate_target_change(total_pegout, remaining_utxos).expect("should not fail");
        assert_eq!(target_change, remaining_utxos);
    }

    #[test]
    fn coin_selection_sanity_checks() {
        let change_script = random_p2tr_keyspend_script();
        let res = coin_selection(
            HashMap::new(),
            HashMap::new(),
            vec![],
            FeeRate::from_sat_per_vb(3).unwrap(),
            change_script.clone(),
        );
        assert_eq!(res.err(), Some(CoinSelectionError::OutputsCannotBeEmpty));

        let output_script = random_p2wpkh_script();
        let res = coin_selection(
            HashMap::new(),
            HashMap::new(),
            vec![(
                TxOut { script_pubkey: output_script, value: Amount::from_sat(1000) },
                create_random_pegout_id(),
            )],
            FeeRate::from_sat_per_vb(3).unwrap(),
            change_script.clone(),
        );
        assert_eq!(
            res.err(),
            Some(CoinSelectionError::CoinSelectionBdk(InsufficientFunds {
                needed: Amount::from_sat(1000),
                available: Amount::from_sat(0),
            }))
        );
    }

    #[test]
    fn test_all_outputs_become_dust() {
        let change_script = random_p2tr_keyspend_script();
        let output_script = random_p2wpkh_script();

        let mut optional_utxos = HashMap::new();
        let utxo = Utxo::new(
            OutPoint::new(random_compute_txid(), 0),
            TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: random_p2tr_keyspend_script(),
            },
            None,
            None,
        );
        optional_utxos.insert(utxo.outpoint, utxo);

        // Create very small outputs that will become dust after high fees
        let outputs = vec![
            (
                TxOut { script_pubkey: output_script.clone(), value: Amount::from_sat(100) },
                create_random_pegout_id(),
            ),
            (
                TxOut { script_pubkey: output_script.clone(), value: Amount::from_sat(150) },
                create_random_pegout_id(),
            ),
            (
                TxOut { script_pubkey: output_script, value: Amount::from_sat(200) },
                create_random_pegout_id(),
            ),
        ];

        // Use a very high fee rate that makes all outputs dust
        let result = coin_selection(
            optional_utxos,
            HashMap::new(),
            outputs,
            FeeRate::from_sat_per_vb(1000).unwrap(), // Very high fee rate
            change_script,
        );

        assert_eq!(result.err(), Some(CoinSelectionError::NoViableOutputs));
    }

    #[test]
    fn test_insufficient_funds() {
        let change_script = random_p2tr_keyspend_script();
        let output_script = random_p2wpkh_script();

        // Create UTXOs with insufficient total value
        let mut optional_utxos = HashMap::new();
        let utxo = Utxo::new(
            OutPoint::new(random_compute_txid(), 0),
            TxOut {
                value: Amount::from_sat(10_000), // Only 10k available
                script_pubkey: random_p2tr_keyspend_script(),
            },
            None,
            None,
        );
        optional_utxos.insert(utxo.outpoint, utxo);

        // Try to create a pegout for more than available
        let outputs = vec![(
            TxOut { script_pubkey: output_script, value: Amount::from_sat(15_000) }, // Need 15k
            create_random_pegout_id(),
        )];

        let result = coin_selection(
            optional_utxos,
            HashMap::new(),
            outputs,
            FeeRate::from_sat_per_vb(1).unwrap(),
            change_script,
        );

        assert_eq!(
            result.err(),
            Some(CoinSelectionError::CoinSelectionBdk(InsufficientFunds {
                needed: Amount::from_sat(15_000),
                available: Amount::from_sat(10_000)
            }))
        );
    }

    #[test]
    fn test_coin_selection_scenarios() {
        let scenarios = create_test_scenarios();

        for scenario in scenarios.iter() {
            let psbt = run_scenario(scenario).unwrap();
            validate_change_calculation(&psbt, scenario);
        }
    }

    #[derive(Debug)]
    struct TestScenario {
        name: String,
        optional_utxo_values_sats: Vec<u64>,   // in sats
        required_utxo_values_sats: Vec<u64>,   // in sats
        pegout_values_sats: Vec<u64>, // in sats
        fee_rate: FeeRate,
        expected_dust_pegout_removed: Vec<u64>,
        expected_change_output_value: u64, // in sats
    }

    fn create_test_scenarios() -> Vec<TestScenario> {
        vec![
            TestScenario {
                name: "change_targets_50%_of_pegout_value".to_string(),
                optional_utxo_values_sats: vec![10_000; 1_000],
                required_utxo_values_sats: vec![],
                pegout_values_sats: vec![100_000, 100_000],
                fee_rate: FeeRate::from_sat_per_kwu(750),
                expected_dust_pegout_removed: vec![],
                expected_change_output_value: 100_000 + 10_000,
            },
            // 100k is 50% of 200k
            // change output has one additional utxo value (10k) as the coin selection algorithm
            // anticipates needing extra to pay fees
            TestScenario {
                name: "change_targets_doesnt_exceed_5%_of_remaining_utxos".to_string(),
                optional_utxo_values_sats: vec![10_000; 60],
                required_utxo_values_sats: vec![],
                pegout_values_sats: vec![100_000, 100_000],
                fee_rate: FeeRate::from_sat_per_kwu(750),
                expected_dust_pegout_removed: vec![],
                expected_change_output_value: 20_000 + 10_000,
                // 20k is 5% of ((60 * 10k) - 200k)
                // change output has one additional utxo value (10k) as the coin selection algorithm
                // anticipates needing extra to pay fees
            },
            TestScenario {
                name: "change_minimum_is_10k".to_string(),
                optional_utxo_values_sats: vec![2_000; 100],
                required_utxo_values_sats: vec![],
                pegout_values_sats: vec![10_000],
                fee_rate: FeeRate::from_sat_per_kwu(750),
                expected_dust_pegout_removed: vec![],
                expected_change_output_value: 10_000 + 2_000,
                // change output has one additional utxo value (2k) as the coin selection algorithm
                // anticipates needing extra to pay fees
            },
            TestScenario {
                name: "multiple_small_outputs".to_string(),
                optional_utxo_values_sats: vec![200_000],
                required_utxo_values_sats: vec![],
                pegout_values_sats: vec![5_000, 8_000, 12_000, 15_000],
                fee_rate: FeeRate::from_sat_per_kwu(750),
                expected_dust_pegout_removed: vec![],
                expected_change_output_value: 160_000,
            },
            TestScenario {
                name: "high_fee_rate".to_string(),
                optional_utxo_values_sats: vec![20_000; 5],
                required_utxo_values_sats: vec![],
                pegout_values_sats: vec![10_000, 30_000],
                fee_rate: FeeRate::from_sat_per_kwu(10_000),
                expected_dust_pegout_removed: vec![],
                expected_change_output_value: 20_000,
            },
            TestScenario {
                name: "1_dust_pegout_to_be_removed".to_string(),
                optional_utxo_values_sats: vec![100_000],
                required_utxo_values_sats: vec![],
                pegout_values_sats: vec![293, 294, 10_000], // 294 is dust threshold for p2wpkh
                fee_rate: FeeRate::from_sat_per_kwu(0),
                expected_dust_pegout_removed: vec![293],
                expected_change_output_value: 89_706,
            },
            TestScenario {
                name: "2_dust_pegouts_to_be_removed".to_string(),
                optional_utxo_values_sats: vec![100_000],
                required_utxo_values_sats: vec![],
                pegout_values_sats: vec![293, 294, 10_000], // 294 is dust threshold for p2wpkh
                fee_rate: FeeRate::from_sat_per_kwu(1000),
                expected_dust_pegout_removed: vec![293, 294],
                expected_change_output_value: 90_000,
            },
        ]
    }

    fn run_scenario(scenario: &TestScenario) -> Result<Psbt, CoinSelectionError> {
        let change_script = random_p2tr_keyspend_script();

        // Create UTXOs
        let optional_utxos = create_utxos_from_values(&scenario.optional_utxo_values_sats);
        let required_utxos = create_utxos_from_values(&scenario.required_utxo_values_sats);

        // Create pegouts. Using p2wpkh scriptpubkey to help identify pegouts in the tx.
        let pegouts: Vec<_> = scenario
            .pegout_values_sats
            .iter()
            .map(|&value_sats| {
                (
                    TxOut {
                        value: Amount::from_sat(value_sats),
                        script_pubkey: random_p2wpkh_scriptpubkey(),
                    },
                    create_random_pegout_id(),
                )
            })
            .collect();

        coin_selection(optional_utxos, required_utxos, pegouts, scenario.fee_rate, change_script)
    }

    fn validate_change_calculation(psbt: &Psbt, scenario: &TestScenario) {
        let tx = psbt.clone().extract_tx().unwrap();

        // Calculate expected values - use actual selected inputs from PSBT
        let total_input_value: u64 = psbt
            .inputs
            .iter()
            .map(|input| input.witness_utxo.as_ref().unwrap().value.to_sat())
            .sum();
        let total_pegout_value: u64 = scenario.pegout_values_sats.iter().sum::<u64>() -
            scenario.expected_dust_pegout_removed.iter().sum::<u64>();
        let remaining_value = total_input_value - total_pegout_value;

        // Find pegout outputs (p2wpkh) vs change output (p2tr)
        let pegout_outputs: Vec<_> =
            tx.output.iter().filter(|o| o.script_pubkey.is_p2wpkh()).collect();
        let change_outputs: Vec<_> =
            tx.output.iter().filter(|o| o.script_pubkey.is_p2tr()).collect();

        assert_eq!(
            change_outputs.len(),
            1,
            "Scenario: {} - Should have exactly one change output",
            scenario.name
        );

        assert_eq!(
            pegout_outputs.len(),
            scenario.pegout_values_sats.len() - scenario.expected_dust_pegout_removed.len(),
            "Scenario: {} - Should have {} pegout outputs",
            scenario.name,
            scenario.pegout_values_sats.len() - scenario.expected_dust_pegout_removed.len()
        );

        let actual_change = change_outputs[0].value.to_sat();

        // Check that no fee is taken from the change output
        assert_eq!(
            actual_change, remaining_value,
            "Scenario: {} - Change value mismatch: expected {}, got {}",
            scenario.name, remaining_value, actual_change
        );

        // Check the change output value to validate the change value targeting
        assert_eq!(
            actual_change, scenario.expected_change_output_value,
            "Scenario: {} - Change targeting mismatch: expected {}, got {}",
            scenario.name, scenario.expected_change_output_value, actual_change
        );

        // assert fee rate is correct
        let actual_fee_rate = calculate_signed_tx_fee_rate(&psbt).expect("should not fail");
        assert_eq!(
            actual_fee_rate, scenario.fee_rate,
            "Scenario: {} - Fee rate mismatch: expected {}, got {}",
            scenario.name, scenario.fee_rate, actual_fee_rate
        );

        let mut psbt_with_signatures = psbt.clone();
        add_dummy_signatures_to_psbt(&mut psbt_with_signatures, TapSighashType::All);
        let fee_rate = psbt_with_signatures.fee_rate().expect("should not fail");
        assert_eq!(fee_rate, scenario.fee_rate);
    }

    fn create_utxos_from_values(values: &[u64]) -> HashMap<OutPoint, Utxo> {
        values
            .iter()
            .enumerate()
            .map(|(i, &value_sats)| {
                let utxo = Utxo::new(
                    OutPoint::new(random_compute_txid(), i as u32),
                    TxOut {
                        value: Amount::from_sat(value_sats),
                        script_pubkey: random_p2tr_keyspend_script(),
                    },
                    None,
                    None,
                );
                (utxo.outpoint, utxo)
            })
            .collect()
    }
}
