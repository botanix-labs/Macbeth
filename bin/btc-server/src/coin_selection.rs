use std::collections::HashMap;

use bdk::{
    psbt::PsbtUtils,
    wallet::coin_selection::{CoinSelectionAlgorithm, Error as BdkCoinselectionError},
};
use bitcoin::{
    psbt::{Error as PsbtError, ExtractTxError, Psbt},
    Amount, FeeRate, OutPoint, ScriptBuf, TxOut,
};
use reth_btc_wallet::TAPROOT_KEYSPEND_SATISFACTION_WEIGHT;

use crate::{database::Utxo, pegout_id::PegoutId, util::OutPointExt, Error};

#[derive(Debug, Error)]
pub enum CoinSelectionError {
    #[error("Coin selection error: {0}")]
    CoinSelectionBdk(#[from] BdkCoinselectionError),
    #[error("PSBT error: {0}")]
    PsbtError(#[from] PsbtError),
    #[error("Extract tx error: {0}")]
    ExtractTxError(#[from] ExtractTxError),
    #[error("Outputs cannot be empty")]
    OutputsCannotBeEmpty,
    #[error("Available utxos cannot be empty")]
    AvailableUtxosCannotBeEmpty,
}

impl PartialEq for CoinSelectionError {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

/// Coin selection
pub(crate) fn coin_selection(
    available_utxos: HashMap<OutPoint, Utxo>,
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
        bdk::WeightedUtxo {
            satisfaction_weight: TAPROOT_KEYSPEND_SATISFACTION_WEIGHT.to_wu() as usize,
            utxo: bdk::Utxo::Local(bdk::LocalOutput {
                outpoint: u.outpoint.to_bdk(),
                txout: bdk::bitcoin::TxOut {
                    script_pubkey: u.output.script_pubkey.to_bytes().into(),
                    value: u.output.value,
                },
                keychain: bdk::KeychainKind::External,
                is_spent: false,
                derivation_index: 0, // we're not using this
                // Also not used
                confirmation_time: bdk::chain::ConfirmationTime::Confirmed { height: 1, time: 1 },
            }),
        }
    };
    let coin_select = bdk::wallet::coin_selection::BranchAndBoundCoinSelection::new(0);

    // Now we're going to hijack BDK coin selection real quick..
    let bdk_utxos = available_utxos.values().map(to_bdk).collect::<Vec<_>>();

    let target_amount = outputs.iter().map(|o| o.0.value).sum::<Amount>();

    // Try once with finalized, then add pending and try again.
    let selection = coin_select
        .coin_select(
            vec![],
            bdk_utxos.clone(),
            fee_rate,
            target_amount.to_sat(),
            &change_script, // drain_script
        )
        .map_err(CoinSelectionError::CoinSelectionBdk)?;

    let selected = selection
        .selected
        .iter()
        .map(|s| available_utxos.get(&OutPoint::from_bdk(s.outpoint())))
        .filter_map(|s| if s.is_some() { s } else { None })
        .collect::<Vec<_>>();

    let change = match selection.excess {
        bdk::wallet::coin_selection::Excess::Change { amount, .. } => {
            Some(TxOut { script_pubkey: change_script.clone(), value: Amount::from_sat(amount) })
        }
        bdk::wallet::coin_selection::Excess::NoChange { .. } => None,
    };

    let mut pegouts = outputs
        .into_iter()
        .map(|(txout, pegout_id)| (txout, pegout_id.as_bytes()))
        .collect::<Vec<_>>();

    let selected_inputs: Vec<reth_btc_wallet::transaction::Input> = selected
        .iter()
        .map(|s| reth_btc_wallet::transaction::Input {
            outpoint: s.outpoint,
            output: s.output.clone(),
            eth_address: s.eth_address,
        })
        .collect();

    let original_psbt = reth_btc_wallet::transaction::create_psbt(
        selected_inputs.clone(),
        pegouts.clone(),
        change.clone(),
    );

    let absolute_fee = Amount::from_sat(original_psbt.fee_amount().expect("no missing any txouts"));
    println!("absolute_fee = {:?}", absolute_fee);
    let fee_per_output = absolute_fee / pegouts.len() as u64;
    println!("fee_per_output = {:?}", fee_per_output);

    for (output, _pegout_id) in pegouts.iter_mut() {
        output.value -= fee_per_output;
    }

    let updated_changed = {
        if let Some(mut ch) = change.clone() {
            ch.value += absolute_fee;
            Some(ch)
        } else {
            None
        }
    };

    let updated_psbt =
        reth_btc_wallet::transaction::create_psbt(selected_inputs, pegouts, updated_changed);

    // Lets extract the tx, doing so will do some fee sanity checks
    // Better to catch them here than later in signing
    let _tx = updated_psbt.clone().extract_tx()?;

    // TODO should check that min relay fee rate is met
    // TODO should check that none of the outputs are now dust

    Ok(updated_psbt)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::{
        coin_selection::CoinSelectionError,
        database::Utxo,
        test_utils::test_utils::{
            create_random_pegout_id, create_tx, random_p2tr_keyspend_script, random_p2wpkh_script,
            setup,
        },
    };
    use bdk::psbt::PsbtUtils;
    use bitcoin::{Amount, FeeRate, OutPoint, TxOut};

    use super::coin_selection;

    // The ideal test case here would iterate over many outputs with varying values
    // checking each time that the fee per output is met and that the change is correct
    // And would also covert the case where change is not needed
    #[test]
    fn coin_selection_sanity_checks() {
        let change_script = random_p2tr_keyspend_script();
        let res = coin_selection(
            HashMap::new(),
            vec![],
            FeeRate::from_sat_per_vb(3).unwrap(),
            change_script.clone(),
        );
        assert_eq!(res.err(), Some(CoinSelectionError::OutputsCannotBeEmpty));

        let output_script = random_p2wpkh_script();
        let res = coin_selection(
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
    fn idk_yet() {
        let app = setup();
        // Add 15 utxos
        let tx = create_tx(15, 1, None);

        let mut utxos = vec![];
        for i in 0..5 {
            let utxo = crate::database::Utxo::new(
                OutPoint::new(tx.input[i].previous_output.txid, i as u32),
                tx.output[0].clone(),
                None,
            );
            utxos.push(utxo.clone());
        }

        let x = utxos.iter().map(|u| u).collect::<Vec<_>>();
        app.add_pegins(&x).expect("add pegins");

        let change_script = random_p2tr_keyspend_script();
        let output_script = random_p2wpkh_script();

        let mut available_utxos: HashMap<OutPoint, Utxo> = HashMap::new();
        for utxo in app.db.get_all_utxos().unwrap() {
            available_utxos.insert(utxo.outpoint, utxo);
        }

        let psbt = coin_selection(
            available_utxos,
            vec![
                (
                    TxOut { script_pubkey: output_script.clone(), value: Amount::from_sat(1000) },
                    create_random_pegout_id(),
                ),
                (
                    TxOut { script_pubkey: output_script, value: Amount::from_sat(1000) },
                    create_random_pegout_id(),
                ),
            ],
            FeeRate::from_sat_per_vb(3).unwrap(),
            change_script,
        )
        .unwrap();
        let tx = psbt.clone().extract_tx().unwrap();
        let fee_rate = psbt.fee_rate();
        println!("fee_rate = {:?}", fee_rate);

        // print inputs
        for input in tx.input {
            println!("input = {:?}", input);
        }
        // print outsputs
        for output in tx.output {
            println!("output = {:?}", output);
        }
    }
}
