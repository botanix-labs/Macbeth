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

    let coin_select = bdk_wallet::coin_selection::LargestFirstCoinSelection;
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
        let tx = create_tx(15, 1, None);
        let change_script = random_p2tr_keyspend_script();
        let output_script = random_p2wpkh_script();
        let mut utxos = vec![];
        for i in 0..5 {
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
        let desired_amount_per_output = Amount::from_sat(1000);
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

        // First some sanity checks
        // if we are requesting 2000 sats we need to spend > 2000 sats to cover the fee
        assert_eq!(tx.input.len(), 3);
        // Ensure we have 2 outputs
        assert_eq!(pegout_outputs.len(), 2);

        // Check that the total output value is less than the total amount being spent
        // i.e some fees are being taken out
        assert!(total_output_value < total_amount_being_spent);

        // Check that the fee per output is correct
        let fee_per_output =
            (total_amount_being_spent - total_output_value) / pegout_outputs.len() as u64;
        for pegout_output in pegout_outputs.iter() {
            assert_eq!(pegout_output.value, desired_amount_per_output - fee_per_output);
        }

        // Check that the change output is correct
        assert_eq!(
            change_output.value,
            total_amount_being_spent - desired_amount_per_output * pegout_outputs.len() as u64
        );
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
