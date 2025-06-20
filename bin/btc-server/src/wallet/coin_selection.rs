use crate::{
    database::version::UtxoVersion,
    wallet::{
        psbt::PegoutId as PegoutIdBytes, SEGWIT_FLAG_WEIGHT, SEGWIT_MARKER_WEIGHT,
        TAPROOT_KEYSPEND_SATISFACTION_WEIGHT,
    },
};
use bdk_wallet::coin_selection::{
    CoinSelectionAlgorithm, InsufficientFunds, OldestFirstCoinSelection,
};
use bitcoin::{
    psbt::{Error as PsbtError, ExtractTxError, Psbt},
    Amount, FeeRate, OutPoint, ScriptBuf, TxOut, Weight,
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
    #[error("Available utxos cannot be empty")]
    AvailableUtxosCannotBeEmpty,
    #[error("Pegout value is smaller than pegout fee")]
    PegoutFeeOverflow,
    #[error("Fee rate overflow")]
    FeeRateOverflow,
    #[error("Sanity check error - SHOULD NOT HAPPEN: {0}")]
    SanityCheckError(#[from] SanityCheckError),
    #[error("No viable outputs after applying fees and filtering dust")]
    NoViableOutputs,
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
    SomeRemaining {
        /// The outputs that remained after filtering
        remaining: Vec<(TxOut, PegoutIdBytes)>,
        /// Number of outputs that were filtered out
        filtered_count: usize,
    },
    /// No outputs were retained after applying fees
    NoneRemaining,
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
    available_utxos: HashMap<OutPoint, Utxo>,
    required_utxos: HashMap<OutPoint, Utxo>,
    outputs: Vec<(TxOut, PegoutId)>,
    fee_rate: FeeRate,
    change_script: ScriptBuf,
) -> Result<Psbt, CoinSelectionError> {
    // Input validation
    if outputs.is_empty() {
        return Err(CoinSelectionError::OutputsCannotBeEmpty);
    }
    if available_utxos.is_empty() {
        return Err(CoinSelectionError::AvailableUtxosCannotBeEmpty);
    }

    // Basic calculations
    let pegouts = outputs
        .into_iter()
        .map(|(txout, pegout_id)| (txout, pegout_id.as_bytes()))
        .collect::<Vec<_>>();
    let total_utxos_value = available_utxos.values().map(|u| u.output.value).sum::<Amount>();
    let total_pegout_target = pegouts.iter().map(|(txout, _)| txout.value).sum::<Amount>();

    // return InsufficientFunds error
    let remaining_utxos_value = total_utxos_value
        .checked_sub(total_pegout_target)
        .ok_or(InsufficientFunds { needed: total_pegout_target, available: total_utxos_value })?;

    // Coin selection using BDK
    let target_change = calculate_target_change(total_pegout_target, remaining_utxos_value);
    let coin_selection_target = total_pegout_target
        .checked_add(target_change)
        .expect("Bitcoin amounts should never overflow u64");
    let coin_selection_algorithm =
        bdk_wallet::coin_selection::BranchAndBoundCoinSelection::new(0, OldestFirstCoinSelection);
    let selected_inputs = perform_coin_selection(
        coin_selection_algorithm,
        available_utxos,
        required_utxos,
        fee_rate,
        coin_selection_target,
        &change_script,
    )?;

    // Calculate change amount, ensuring that change pays no fees
    let total_selected_inputs = selected_inputs.iter().map(|i| i.output.value).sum::<Amount>();
    let final_change_amount = total_selected_inputs
        .checked_sub(total_pegout_target)
        .expect("Coin selection should at least cover the pegout target");
    let change = Some(TxOut { script_pubkey: change_script.clone(), value: final_change_amount });

    let psbt = apply_fees_and_create_psbt(&selected_inputs, pegouts, change, fee_rate)?;

    sanity_check_psbt(&psbt, &selected_inputs, change_script, total_pegout_target)?;

    Ok(psbt)
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
    if change_output_value
        != total_input_value
            .checked_sub(total_pegout_target)
            .expect("Bitcoin amounts should never overflow u64")
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

fn perform_coin_selection<T: CoinSelectionAlgorithm>(
    coin_selection_algorithm: T,
    available_utxos: HashMap<OutPoint, Utxo>,
    required_utxos: HashMap<OutPoint, Utxo>,
    fee_rate: FeeRate,
    coin_selection_target: Amount,
    change_script: &ScriptBuf,
) -> Result<Vec<crate::wallet::psbt::InputDTO>, CoinSelectionError> {
    let mut rng = rand::thread_rng();

    let selection = coin_selection_algorithm
        .coin_select(
            required_utxos.values().map(utxo_to_bdk).collect::<Vec<_>>(),
            available_utxos.values().map(utxo_to_bdk).collect::<Vec<_>>(),
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
        .map(|s| available_utxos.get(&OutPoint::from_bdk(s.outpoint())))
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
    change: Option<TxOut>,
    fee_rate: FeeRate,
) -> Result<Psbt, CoinSelectionError> {
    let absolute_fee = calculate_signed_tx_fee(&selected_inputs, &pegouts, &change, fee_rate)?;
    let first_attempt = try_apply_fees_and_filter_dust(pegouts.clone(), absolute_fee);

    match first_attempt {
        FilterResult::AllRemaining(final_pegouts) => {
            // No outputs were filtered out, we can return the psbt
            Ok(crate::wallet::psbt::create_psbt(selected_inputs.to_vec(), final_pegouts, change))
        }
        FilterResult::SomeRemaining { remaining, filtered_count } => {
            debug!("Filtered out {} outputs due to dust or insufficient funds", filtered_count);

            // Some outputs were filtered out, we need to re-calculate the fee
            let temp_absolute_fee =
                calculate_signed_tx_fee(&selected_inputs, &remaining, &change, fee_rate)?;
            let second_attempt = try_apply_fees_and_filter_dust(pegouts, temp_absolute_fee);

            match second_attempt {
                FilterResult::AllRemaining(final_outputs) => Ok(crate::wallet::psbt::create_psbt(
                    selected_inputs.to_vec(),
                    final_outputs,
                    change,
                )),
                _ => {
                    // should never happen
                    return Err(SanityCheckError::RecalculationFilteredMoreOutputs.into());
                }
            }
        }
        FilterResult::NoneRemaining => {
            // All outputs were filtered out due to dust
            return Err(CoinSelectionError::NoViableOutputs);
        }
    }
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

/// Calculate the absolute fee for a transaction given its inputs and outputs
fn calculate_signed_tx_fee(
    inputs: &[crate::wallet::psbt::InputDTO],
    outputs: &[(TxOut, PegoutIdBytes)],
    change: &Option<TxOut>,
    fee_rate: FeeRate,
) -> Result<Amount, CoinSelectionError> {
    let psbt_without_fees =
        crate::wallet::psbt::create_psbt(inputs.to_vec(), outputs.to_vec(), change.clone());
    let unsigned_tx_weight = psbt_without_fees.unsigned_tx.weight();

    let per_input_witness_item_count = Weight::from_wu(1);
    let total_signature_weight = (TAPROOT_KEYSPEND_SATISFACTION_WEIGHT
        + per_input_witness_item_count)
        .checked_mul(inputs.len() as u64)
        .expect("Bitcoin amounts should never overflow u64");

    let total_weight =
        unsigned_tx_weight + total_signature_weight + SEGWIT_FLAG_WEIGHT + SEGWIT_MARKER_WEIGHT;
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
fn calculate_target_change(total_pegout_value: Amount, remaining_utxos_value: Amount) -> Amount {
    // default target change is a percentage of the total pegout value
    let mut target_change = total_pegout_value
        .checked_mul(TARGET_CHANGE_PERCENT)
        .expect("Bitcoin amounts should never overflow u64")
        .checked_div(100)
        .expect("Division by 100 should never fail");

    let max_change_value = remaining_utxos_value
        .checked_mul(MAX_CHANGE_PERCENT)
        .expect("Bitcoin amounts should never overflow u64")
        .checked_div(100)
        .expect("Division by 100 should never fail");
    if target_change > max_change_value {
        target_change = max_change_value;
    }

    // for small pegouts, set the change is at least the minimum change amount
    let min_change_value = Amount::from_sat(MIN_CHANGE_SATS);
    if target_change < min_change_value {
        target_change = min_change_value;
    }

    // this is an edge case, probably only relevant for test cases,
    // if the minimum is more than the remaining utxos just return the remaining utxos value
    if target_change > remaining_utxos_value {
        target_change = remaining_utxos_value;
    }

    target_change
}

fn try_apply_fees_and_filter_dust(
    pegouts: Vec<(TxOut, PegoutIdBytes)>,
    absolute_fee: Amount,
) -> FilterResult {
    if pegouts.is_empty() {
        return FilterResult::NoneRemaining;
    }

    let original_count = pegouts.len();
    let fees_to_subtract = calculate_fee_distribution(&pegouts, absolute_fee);

    let mut result = Vec::new();
    for (i, (txout, pegout_id)) in pegouts.into_iter().enumerate() {
        let script_pubkey = txout.script_pubkey.clone();
        let original_value = txout.value;

        if let Some(new_value) = txout.value.checked_sub(fees_to_subtract[i]) {
            let updated_output = TxOut { value: new_value, ..txout };

            let dust_threshold = updated_output.script_pubkey.minimal_non_dust();
            if updated_output.value >= dust_threshold {
                result.push((updated_output, pegout_id));
            } else {
                debug!("Filtered out pegout output due to dust: output_script={:?}, output_value={:?}, fee_to_subtract={:?}", 
                script_pubkey, original_value, fees_to_subtract[i]);
            }
        } else {
            debug!("Filtered out pegout output due to insufficient funds: output_script={:?}, output_value={:?}, fee_to_subtract={:?}", 
            script_pubkey, original_value, fees_to_subtract[i]);
        }
    }

    if result.len() == original_count {
        FilterResult::AllRemaining(result)
    } else if result.is_empty() {
        FilterResult::NoneRemaining
    } else {
        let result_len = result.len();
        FilterResult::SomeRemaining {
            remaining: result,
            filtered_count: original_count - result_len,
        }
    }
}

fn calculate_fee_distribution(
    pegouts: &[(TxOut, PegoutIdBytes)],
    absolute_fee: Amount,
) -> Vec<Amount> {
    let num_outputs = pegouts.len();
    let base_fee_per_output = absolute_fee
        .checked_div(num_outputs as u64)
        .expect("Number of pegouts should never be zero");
    let remainder = absolute_fee % num_outputs as u64;

    let mut fees_to_subtract = vec![base_fee_per_output; num_outputs];

    // Distribute the remainder one sat at a time to highest value outputs
    let mut sorted_indices: Vec<usize> = (0..num_outputs).collect();
    sorted_indices.sort_by_key(|&index| Reverse(pegouts[index].0.value));

    for sat_index in 0..remainder.to_sat() as usize {
        let index = sorted_indices[sat_index];
        fees_to_subtract[index] = fees_to_subtract[index]
            .checked_add(Amount::from_sat(1))
            .expect("Fee calculation should not overflow adding single satoshi");
    }
    fees_to_subtract
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::{
        database::Utxo,
        test_utils::{
            create_random_pegout_id, create_tx, random_p2tr_keyspend_script, random_p2wpkh_script,
            setup_db,
        },
        wallet::coin_selection::CoinSelectionError,
    };
    use bdk_wallet::psbt::PsbtUtils;
    use bitcoin::{Amount, FeeRate, OutPoint, TxOut};

    use super::coin_selection;

    // The ideal test case here would iterate over many outputs with varying values
    // checking each time that the fee per output is met and that the change is correct
    // And would also cover the case where change is not needed
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
        assert_eq!(res.err(), Some(CoinSelectionError::AvailableUtxosCannotBeEmpty));
    }

    #[test]
    fn should_take_fee_out_of_outputs() {
        let (db, _) = setup_db();
        // Add 15 utxos
        let tx = create_tx(100, 1, None);
        let change_script = random_p2tr_keyspend_script();
        let output_script = random_p2wpkh_script();
        let mut utxos = vec![];
        for i in 0..100 {
            let utxo = crate::database::Utxo::new(
                OutPoint::new(tx.input[i].previous_output.txid, i as u32),
                // Each prevout has a value of 1000 sats
                tx.output[0].clone(),
                None,
                None,
            );
            utxos.push(utxo.clone());
        }

        let available_utxos = utxos.iter().map(|u| u).collect::<Vec<_>>();
        db.store_utxos(&available_utxos).expect("add pegins");

        let mut available_utxos: HashMap<OutPoint, Utxo> = HashMap::new();
        for utxo in db.get_all_utxos().unwrap() {
            available_utxos.insert(utxo.outpoint, utxo);
        }

        let required_utxos = HashMap::new();
        let desired_fee_rate = FeeRate::from_sat_per_vb(3).unwrap();
        let desired_amount_per_output = Amount::from_sat(10000);
        let psbt = coin_selection(
            available_utxos,
            required_utxos,
            vec![
                (
                    TxOut {
                        script_pubkey: output_script.clone(),
                        value: desired_amount_per_output,
                    },
                    create_random_pegout_id(),
                ),
                (
                    TxOut { script_pubkey: output_script, value: desired_amount_per_output },
                    create_random_pegout_id(),
                ),
            ],
            desired_fee_rate,
            change_script.clone(),
        )
        .unwrap();
        let tx = psbt.clone().extract_tx().unwrap();
        let fee_rate = psbt.fee_rate().unwrap();
        println!("fee_rate = {:?}", fee_rate);
        println!("desired_fee_rate = {:?}", desired_fee_rate);
        // make_tx is hardcoded to have 1000 sats per output
        let total_amount_being_spent = Amount::from_sat((tx.input.len() * 1000) as u64);
        let pegout_outputs =
            tx.output.iter().filter(|o| o.script_pubkey != change_script).collect::<Vec<_>>();
        let change_output = tx.output.iter().find(|o| o.script_pubkey == change_script).unwrap();
        let change_output_value = change_output.value;
        let pegout_outputs_value = pegout_outputs.iter().map(|o| o.value).sum::<Amount>();
        let total_output_value = pegout_outputs_value + change_output_value;

        assert_eq!(tx.input.len(), 37);
        assert_eq!(pegout_outputs.len(), 2);

        // Check that the total output value is less than the total amount being spent
        // i.e some fees are being taken out
        assert!(total_output_value < total_amount_being_spent);

        // Check that the change output is correct
        assert_eq!(
            change_output.value,
            total_amount_being_spent - desired_amount_per_output * pegout_outputs.len() as u64
        );
    }

    #[test]
    fn test_no_viable_outputs() {
        let change_script = random_p2tr_keyspend_script();
        let output_script = random_p2wpkh_script();

        // Create inputs with sufficient value
        let input_value = Amount::from_sat(40_000);
        let input_script = random_p2tr_keyspend_script();

        // Create the TxOut that create_tx will use
        let tx_output_template = TxOut { script_pubkey: input_script.clone(), value: input_value };

        let utxo1_tx = create_tx(1, 1, Some(tx_output_template.clone()));
        let utxo1 = Utxo::new(
            OutPoint::new(utxo1_tx.compute_txid(), 0),
            tx_output_template.clone(),
            None,
            None,
        );
        let utxo2_tx = create_tx(1, 1, Some(tx_output_template.clone()));
        let utxo2 =
            Utxo::new(OutPoint::new(utxo2_tx.compute_txid(), 0), tx_output_template, None, None);

        let mut available_utxos = HashMap::new();
        available_utxos.insert(utxo1.outpoint, utxo1);
        available_utxos.insert(utxo2.outpoint, utxo2);

        // Create an output with a small value
        let output_value = Amount::from_sat(500);
        let outputs = vec![(
            TxOut { script_pubkey: output_script, value: output_value },
            create_random_pegout_id(),
        )];

        // Set a high fee rate
        let fee_rate = FeeRate::from_sat_per_vb(100).unwrap();

        let required_utxos = HashMap::new();
        let result = coin_selection(
            available_utxos,
            required_utxos,
            outputs,
            fee_rate,
            change_script.clone(),
        );

        assert_eq!(result.err(), Some(CoinSelectionError::NoViableOutputs));
    }

    #[test]
    fn should_consider_large_outputs() {
        let (db, _) = setup_db();
        // Add 15 utxos
        let tx = create_tx(15, 1, None);
        let change_script = random_p2tr_keyspend_script();
        let output_script = random_p2wpkh_script();
        let mut utxos = vec![];
        for i in 0..5 {
            let utxo = crate::database::Utxo::new(
                OutPoint::new(tx.input[i].previous_output.txid, i as u32),
                TxOut {
                    value: Amount::from_sat(40_000),
                    script_pubkey: random_p2tr_keyspend_script(),
                },
                None,
                None,
            );
            utxos.push(utxo.clone());
        }

        let available_utxos = utxos.iter().map(|u| u).collect::<Vec<_>>();
        db.store_utxos(&available_utxos).expect("add pegins");

        let mut available_utxos: HashMap<OutPoint, Utxo> = HashMap::new();
        for utxo in db.get_all_utxos().unwrap() {
            available_utxos.insert(utxo.outpoint, utxo);
        }

        let mut outputs = vec![];
        for _ in 0..123 {
            outputs.push((
                TxOut { script_pubkey: output_script.clone(), value: Amount::from_sat(1000) },
                create_random_pegout_id(),
            ));
        }

        let required_utxos = HashMap::new();
        let desired_fee_rate = FeeRate::from_sat_per_vb(3).unwrap();

        let psbt = coin_selection(
            available_utxos,
            required_utxos,
            outputs,
            desired_fee_rate,
            change_script.clone(),
        )
        .unwrap();

        let tx = psbt.clone().extract_tx().unwrap();
        let fee_rate = psbt.fee_rate().unwrap();

        let pegout_outputs =
            tx.output.iter().filter(|o| o.script_pubkey != change_script).collect::<Vec<_>>();

        assert_eq!(tx.input.len(), 4);
        assert_eq!(pegout_outputs.len(), 123);

        // NOTE: Slight inconsistency here, described more in the
        // `coin_selection` function.
        assert_eq!(fee_rate, FeeRate::from_sat_per_kwu(761));
        assert_eq!(desired_fee_rate, FeeRate::from_sat_per_kwu(750));
    }
}
