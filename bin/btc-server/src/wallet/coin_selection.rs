use crate::{
    database::version::UtxoVersion,
    wallet::{psbt::PegoutId as PegoutIdBytes, TAPROOT_KEYSPEND_SATISFACTION_WEIGHT},
};
use bdk_wallet::coin_selection::{
    CoinSelectionAlgorithm, InsufficientFunds, OldestFirstCoinSelection,
};
use bitcoin::{
    psbt::{Error as PsbtError, ExtractTxError, Psbt},
    Amount, FeeRate, OutPoint, ScriptBuf, TxOut, Weight,
};
use std::collections::HashMap;
use thiserror::Error;

use crate::{database::Utxo, pegout_id::PegoutId, util::OutPointExt};

const TAPROOT_OUTPUT_DUST_THRESHOLD: Amount = Amount::from_sat(330);

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
}

#[derive(Debug, Error)]
pub enum SanityCheckError {
    #[error("Absolute fee not evenly distributed among pegouts")]
    /// Failed validation: `total_pegout_value + absolute_fee == target_amount`
    AbsoluteFeeNotDistributed {
        total_pegout_value: Amount,
        absolute_fee: Amount,
        target_amount: Amount,
    },
    #[error("Bad refund balance in change output")]
    /// Failed validation: `change_output_value == total_input_value - target_amount`
    BadRefundBalance {
        change_output_value: Amount,
        total_input_value: Amount,
        target_amount: Amount,
    },
}

impl PartialEq for CoinSelectionError {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

/// Coin selection
pub(crate) fn coin_selection(
    available_utxos: HashMap<OutPoint, Utxo>,
    required_utxos: HashMap<OutPoint, Utxo>,
    outputs: Vec<(TxOut, PegoutId)>,
    fee_rate: FeeRate,
    change_script: ScriptBuf,
) -> Result<Psbt, CoinSelectionError> {
    // Perform some sanity checks
    if outputs.is_empty() {
        return Err(CoinSelectionError::OutputsCannotBeEmpty);
    }
    if available_utxos.is_empty() {
        return Err(CoinSelectionError::AvailableUtxosCannotBeEmpty);
    }

    let to_bdk = |u: &Utxo| {
        bdk_wallet::WeightedUtxo {
            satisfaction_weight: TAPROOT_KEYSPEND_SATISFACTION_WEIGHT,
            utxo: bdk_wallet::Utxo::Local(bdk_wallet::LocalOutput {
                outpoint: u.outpoint.to_bdk(),
                txout: bdk_wallet::bitcoin::TxOut {
                    script_pubkey: u.output.script_pubkey.to_bytes().into(),
                    value: u.output.value,
                },
                keychain: bdk_wallet::KeychainKind::External,
                is_spent: false,
                derivation_index: 0, // we're not using this
                // Also not used
                chain_position: bdk_wallet::chain::ChainPosition::Confirmed {
                    anchor: bdk_wallet::chain::ConfirmationBlockTime::default(),
                    transitively: None,
                },
            }),
        }
    };

    // NOTE (lamafab): The coin selection algorithm that we use does not appear
    // to consider output weights, nor base weights, at all. We hence compute
    // the estimated fee for all outputs and make that part of the
    // `target_amount` later on, with the goal that the coin selection algorithm
    // includes the necessary amount of inputs to cover the final fee. We just
    // ignore the base weights.

    // Calculate the weight of all the outputs. Do note that we allow different
    // transaction variants, be it P2SH, P2WPKH, etc.
    let estimated_output_weight: Weight =
        // Apply pegouts.
        outputs.iter().fold(Weight::ZERO, |acc, (tx_out, _)| acc + tx_out.weight())
        // Apply change output (P2TR), which _might_ not be set.
        + Weight::from_wu(172);

    let estimated_output_fee =
        fee_rate.fee_wu(estimated_output_weight).ok_or(CoinSelectionError::FeeRateOverflow)?;

    let coin_select =
        bdk_wallet::coin_selection::BranchAndBoundCoinSelection::new(0, OldestFirstCoinSelection);

    let target_amount = outputs.iter().map(|o| o.0.value).sum::<Amount>();

    let mut rng = rand::thread_rng();
    // Try once with finalized, then add pending and try again.
    let selection = coin_select
        .coin_select(
            required_utxos.values().map(to_bdk).collect::<Vec<_>>(),
            available_utxos.values().map(to_bdk).collect::<Vec<_>>(),
            fee_rate,
            // Include the estimated fee for the outputs
            target_amount + estimated_output_fee,
            &change_script, // drain_script
            &mut rng,
        )
        .map_err(CoinSelectionError::CoinSelectionBdk)?;

    let selected = selection
        .selected
        .iter()
        .map(|s| available_utxos.get(&OutPoint::from_bdk(s.outpoint())))
        .filter_map(|s| if s.is_some() { s } else { None })
        .collect::<Vec<_>>();

    let change = match selection.excess {
        bdk_wallet::coin_selection::Excess::Change { amount, .. } => {
            Some(TxOut { script_pubkey: change_script.clone(), value: amount })
        }
        bdk_wallet::coin_selection::Excess::NoChange { .. } => None,
    };

    let pegouts = outputs
        .into_iter()
        .map(|(txout, pegout_id)| (txout, pegout_id.as_bytes()))
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

    let original_psbt =
        crate::wallet::psbt::create_psbt(selected_inputs.clone(), pegouts.clone(), change.clone());

    // NOTE (lamafab): This fee calculation does not respect the passed on
    // `fee_rate`. Technically, this absolute fee should be adjusted such that:
    //
    // > absolute_fee = fee_rate * original_psbt.unsigned_tx.weight()
    //
    // But we will keep it simple for now without messing with the original
    // implementation. We can revisit this later.
    let absolute_fee = original_psbt.fee().expect("not missing any txouts");
    let fee_per_output = absolute_fee / pegouts.len() as u64;

    let pegouts = pegouts
        .into_iter()
        .filter_map(|(mut output, _pegout_id)| {
            match output
                .value
                .checked_sub(fee_per_output)
                .ok_or_else(|| CoinSelectionError::PegoutFeeOverflow)
            {
                Ok(amount) => {
                    output.value = amount;
                    Some((output, _pegout_id))
                }
                Err(_) => {
                    // Ignore the pegout
                    None
                }
            }
        })
        .collect::<Vec<(TxOut, PegoutIdBytes)>>();

    let updated_changed = {
        if let Some(mut ch) = change.clone() {
            ch.value += absolute_fee;
            Some(ch)
        } else if absolute_fee > TAPROOT_OUTPUT_DUST_THRESHOLD {
            Some(TxOut { script_pubkey: change_script.clone(), value: absolute_fee })
        } else {
            None
        }
    };

    let change_available = updated_changed.is_some();

    // The total input value, as selected by the coin selection algorithm.
    let total_input_value =
        selected_inputs.iter().fold(Amount::ZERO, |acc, utxo| acc + utxo.output.value);

    // The total pegouts value, excluding the change output, and with the
    // absolute fee evenly distributed among each pegouts.
    let total_pegout_value = pegouts.iter().map(|(txout, _)| txout.value).sum::<Amount>();

    let updated_psbt = crate::wallet::psbt::create_psbt(selected_inputs, pegouts, updated_changed);

    // Lets extract the tx, doing so will do some fee sanity checks
    // Better to catch them here than later in signing
    let tx = updated_psbt.clone().extract_tx()?;

    {
        let absolute_fee = updated_psbt.fee().expect("not missing any txouts");

        // VALIDATE: Total pegout value plus absolute fee must be equal to target amount
        if total_pegout_value + absolute_fee != target_amount {
            return Err(SanityCheckError::AbsoluteFeeNotDistributed {
                total_pegout_value,
                absolute_fee,
                target_amount,
            }
            .into());
        }

        // VALIDATE: Change output value must be equal the total input value
        // minus the target amount. Notably, the pegouts cover all the fees.
        if change_available {
            let change_output = tx.output.last().expect("change output not included");
            let change_output_value = change_output.value;

            if change_output_value != total_input_value - target_amount {
                return Err(SanityCheckError::BadRefundBalance {
                    change_output_value,
                    total_input_value,
                    target_amount,
                }
                .into());
            }
        }
    }

    Ok(updated_psbt)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::{
        database::Utxo,
        test_utils::{
            create_random_pegout_id, create_tx, random_compute_txid, random_p2tr_keyspend_script,
            random_p2wpkh_script, random_p2wpkh_scriptpubkey,
        },
        wallet::coin_selection::CoinSelectionError,
    };
    use bitcoin::{Amount, FeeRate, OutPoint, Psbt, TxOut};

    use super::coin_selection;

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
    fn test_pegout_fee_overflow() {
        let change_script = random_p2tr_keyspend_script();
        let output_script = random_p2wpkh_script();

        // Create inputs with sufficient value
        let input_value = Amount::from_sat(10_000);
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

        // Assert that the response is ok and we have filtered the outputs to 1
        assert!(result.is_ok());
        assert!(result.unwrap().outputs.len() == 1);
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
        utxo_values_sats: Vec<u64>,   // in sats
        pegout_values_sats: Vec<u64>, // in sats
        fee_rate: FeeRate,
        expected_dust_pegout_removed: Vec<u64>,
    }

    fn create_test_scenarios() -> Vec<TestScenario> {
        vec![
            // Simple case: plenty of funds, low fee
            TestScenario {
                utxo_values_sats: vec![100_000, 50_000, 20_000, 10_000],
                pegout_values_sats: vec![10_000, 20_000],
                fee_rate: FeeRate::from_sat_per_kwu(750),
                expected_dust_pegout_removed: vec![],
            },
            // Multiple small outputs
            TestScenario {
                utxo_values_sats: vec![200_000],
                pegout_values_sats: vec![5_000, 8_000, 12_000, 15_000],
                fee_rate: FeeRate::from_sat_per_kwu(750),
                expected_dust_pegout_removed: vec![],
            },
            // High fee rate
            TestScenario {
                utxo_values_sats: vec![100_000, 80_000],
                pegout_values_sats: vec![25_000, 30_000],
                fee_rate: FeeRate::from_sat_per_kwu(10_000),
                expected_dust_pegout_removed: vec![],
            },
        ]
    }

    fn run_scenario(scenario: &TestScenario) -> Result<Psbt, CoinSelectionError> {
        let change_script = random_p2tr_keyspend_script();

        // Create UTXOs
        let mut available_utxos = HashMap::new();
        for (i, &value_sats) in scenario.utxo_values_sats.iter().enumerate() {
            let utxo = Utxo::new(
                OutPoint::new(random_compute_txid(), i as u32),
                TxOut {
                    value: Amount::from_sat(value_sats),
                    script_pubkey: random_p2tr_keyspend_script(),
                },
                None,
                None,
            );
            available_utxos.insert(utxo.outpoint, utxo);
        }

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

        let required_utxos = HashMap::new();

        coin_selection(available_utxos, required_utxos, pegouts, scenario.fee_rate, change_script)
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
        let expected_change = total_input_value - total_pegout_value;

        // Find pegout outputs (p2wpkh) vs change output (p2tr)
        let pegout_outputs: Vec<_> =
            tx.output.iter().filter(|o| o.script_pubkey.is_p2wpkh()).collect();
        let change_outputs: Vec<_> =
            tx.output.iter().filter(|o| o.script_pubkey.is_p2tr()).collect();

        assert_eq!(change_outputs.len(), 1, "Should have exactly one change output");

        assert_eq!(
            pegout_outputs.len(),
            scenario.pegout_values_sats.len() - scenario.expected_dust_pegout_removed.len(),
            "Should have {} pegout outputs",
            scenario.pegout_values_sats.len() - scenario.expected_dust_pegout_removed.len()
        );

        let actual_change = change_outputs[0].value.to_sat();

        assert_eq!(
            actual_change, expected_change,
            "Change mismatch: expected {}, got {}",
            expected_change, actual_change
        );

        // TODO: add this back in when the fee rate is fixed

        // // assert fee rate is correct
        // let actual_fee_rate = calculate_signed_tx_fee_rate(&psbt);
        // assert_eq!(
        //     actual_fee_rate, scenario.fee_rate,
        //     "Fee rate mismatch: expected {}, got {}",
        //     scenario.fee_rate, actual_fee_rate
        // );
    }
}
