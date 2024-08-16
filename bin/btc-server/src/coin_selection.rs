use std::collections::HashMap;

use bdk::wallet::coin_selection::{CoinSelectionAlgorithm, Error as BdkCoinselectionError};
use bitcoin::{psbt::Psbt, Amount, FeeRate, OutPoint, ScriptBuf, Transaction, TxOut};
use reth_btc_wallet::{psbt::PsbtInputExt, TAPROOT_KEYSPEND_SATISFACTION_WEIGHT};

use crate::{database::Utxo, pegout_id::PegoutId, util::OutPointExt, Error};

pub trait TransactionExt {
    fn calculate_absolute_fee(&self, fee_rate: &FeeRate) -> Amount;
    fn calculate_fee_per_output(&self, fee_rate: &FeeRate) -> Amount;
}

impl TransactionExt for Transaction {
    fn calculate_absolute_fee(&self, fee_rate: &FeeRate) -> Amount {
        let vsize = self.vsize();
        let fee = fee_rate.to_sat_per_vb_ceil() * vsize as u64;
        Amount::from_sat(fee)
    }

    fn calculate_fee_per_output(&self, fee_rate: &FeeRate) -> Amount {
        let absolute_fee = self.calculate_absolute_fee(fee_rate);
        let fee_per_output = absolute_fee / self.output.len() as u64;
        fee_per_output
    }
}

#[derive(Debug, Error)]
pub enum CoinSelectionError {
    #[error("Coin selection error: {0}")]
    CoinSelectionBdk(#[from] BdkCoinselectionError),
}

/// Coin selection
pub(crate) fn coin_selection(
    available_utxos: HashMap<OutPoint, Utxo>,
    outputs: Vec<(TxOut, Option<PegoutId>)>,
    fee_rate: FeeRate,
    change_script: ScriptBuf,
) -> Result<Psbt, CoinSelectionError> {
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

    // Now we're going to hijack BDK coin selection real quick..
    let bdk_utxos = available_utxos.values().map(to_bdk).collect::<Vec<_>>();

    let coin_select = bdk::wallet::coin_selection::BranchAndBoundCoinSelection::new(0);
    let target_amount = outputs.iter().map(|o| o.0.value).sum::<Amount>();

    // Try once with finalized, then add pending and try again.
    let selection = coin_select
        .coin_select(
            vec![],
            bdk_utxos.clone(),
            // we dont want to pay any fee out of the change output
            FeeRate::ZERO,
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

    let pegouts = outputs
        .into_iter()
        .map(|(txout, pegout_id)| {
            if let Some(pegout_id) = pegout_id {
                (txout, Some(pegout_id.as_bytes()))
            } else {
                (txout, None)
            }
        })
        .collect::<Vec<_>>();

    let original_psbt = reth_btc_wallet::transaction::create_psbt(
        selected
            .iter()
            .map(|s| reth_btc_wallet::transaction::Input {
                outpoint: s.outpoint,
                output: s.output.clone(),
                eth_address: s.eth_address,
            })
            .collect(),
        pegouts.clone(),
        Some(TxOut {
            script_pubkey: change_script.clone(),
            // TODO remove placeholder value
            value: Amount::from_sat(550),
        }),
    );

    let vsize = original_psbt.clone().extract_tx_unchecked_fee_rate().vsize();
    println!("vsize = {:?}", vsize);

    let fee_per_output =
        Amount::from_sat((fee_rate.to_sat_per_vb_ceil() * vsize as u64) / pegouts.len() as u64);
    println!("fee_per_output = {:?}", fee_per_output);
    println!("target_amount = {:?}", target_amount);
    let mut tx = original_psbt.clone().extract_tx_unchecked_fee_rate();
    // substract fee from non-change outputs
    for output in tx.output.iter_mut() {
        if output.script_pubkey != change_script {
            output.value -= fee_per_output;
        }
    }

    let amount_left_over = tx.output.iter().map(|o| o.value).sum::<Amount>() - target_amount;
    println!("amount_left_over = {:?}", amount_left_over);

    let fees = Psbt::from_unsigned_tx(tx).unwrap().fee().unwrap();
    println!("fees = {:?}", fees);

    // TODO should check that min relay fee rate is met
    // TODO should check that none of the outputs are now dust

    Ok(original_psbt)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use bitcoin::{secp256k1::SECP256K1, Amount, FeeRate, OutPoint, TxOut};

    use rand::rngs::OsRng;
    use reth_btc_wallet::address::generate_taproot_change_scriptpubkey;

    use crate::{
        coin_selection::TransactionExt,
        database::Utxo,
        test_utils::test_utils::{create_random_pegout_id, create_tx, setup},
    };

    use super::coin_selection;

    #[test]
    fn test_calculate_abolute_fee() {
        let tx = create_tx(1, 1, None);
        let fee_rate = FeeRate::from_sat_per_vb(3).unwrap();
        let fee = tx.calculate_absolute_fee(&fee_rate);
        // TODO assertions
    }

    #[test]
    fn test_calculate_fee_per_output() {
        let tx = create_tx(1, 1, None);
        let fee_rate = FeeRate::from_sat_per_vb(3).unwrap();
        let fee = (&tx).calculate_fee_per_output(&fee_rate);
        println!("fee = {:?}", fee);
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

        let key_pair = bitcoin::secp256k1::generate_keypair(&mut OsRng);
        let change_script = generate_taproot_change_scriptpubkey(&key_pair.1);

        let pk = bitcoin::PublicKey::from_private_key(
            SECP256K1,
            &bitcoin::PrivateKey::generate(bitcoin::Network::Regtest),
        );
        let output_script =
            bitcoin::Address::p2wpkh(&pk, bitcoin::Network::Regtest).unwrap().script_pubkey();

        let mut available_utxos: HashMap<OutPoint, Utxo> = HashMap::new();
        for utxo in app.db.get_all_utxos().unwrap() {
            available_utxos.insert(utxo.outpoint, utxo);
        }

        let pbst = coin_selection(
            available_utxos,
            vec![
                (
                    TxOut { script_pubkey: output_script.clone(), value: Amount::from_sat(1_000) },
                    Some(create_random_pegout_id()),
                ),
                (
                    TxOut { script_pubkey: output_script, value: Amount::from_sat(1_000) },
                    Some(create_random_pegout_id()),
                ),
            ],
            FeeRate::from_sat_per_vb(3).unwrap(),
            change_script,
        )
        .unwrap();
        let tx = pbst.extract_tx().unwrap();

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
