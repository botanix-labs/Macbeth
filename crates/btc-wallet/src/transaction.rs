use bitcoin::{
    psbt::Psbt,
    sighash::{TapSighash, TapSighashType},
    OutPoint, TxOut,
};

use crate::psbt::{PegoutId, PsbtInputExt, PsbtOutputExt};

/// Utxo DTO struct
#[derive(Debug, Clone)]
pub struct Input {
    pub outpoint: OutPoint,
    pub output: TxOut,
    pub eth_address: Option<[u8; 20]>,
}

/// Create psbt with proprietary tweak fields
pub fn create_psbt(
    inputs: Vec<Input>,
    outputs: Vec<(TxOut, PegoutId)>,
    change: Option<TxOut>,
) -> Psbt {
    let mut output: Vec<TxOut> = outputs.iter().map(|(out, _)| out).cloned().collect();
    if let Some(change) = change {
        output.push(change);
    }
    let tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::locktime::absolute::LockTime::ZERO,
        input: inputs
            .iter()
            .map(|u| bitcoin::TxIn {
                previous_output: u.outpoint,
                sequence: bitcoin::Sequence::MAX,
                script_sig: bitcoin::ScriptBuf::new(),
                witness: Default::default(),
            })
            .collect(),
        output,
    };

    // Create PSBT
    // add input meta
    let mut psbt = Psbt::from_unsigned_tx(tx).expect("tx is unsigned");
    for (psbt_input, utxo) in psbt.inputs.iter_mut().zip(inputs.iter()) {
        psbt_input.witness_utxo = Some(utxo.output.clone());
        if let Some(eth_addr) = utxo.eth_address {
            psbt_input.set_eth_address(eth_addr);
        }
    }

    // add output meta
    for (psbt_output, (_out, pegout_id)) in psbt.outputs.iter_mut().zip(outputs.iter()) {
        // Pegout ids are stored in the proprietary field to be checked and validated
        // by peers
        psbt_output.set_pegout_id(*pegout_id);
    }

    psbt
}

#[derive(Debug, thiserror::Error)]
pub enum CalculateSighashError {
    #[error("Failed to calculate sighash: {0}")]
    FailedToCalculateSighash(#[from] bitcoin::sighash::Error),
    #[error("Missing witness utxo")]
    MissingWitnessUtxo,
}

/// Calculate the sighash for a taproot keyspend
/// Using tapsighash type ALL
pub fn calculate_sighash(
    psbt: &Psbt,
    input_index: usize,
) -> Result<TapSighash, CalculateSighashError> {
    let mut sighashcache = bitcoin::sighash::SighashCache::new(&psbt.unsigned_tx);
    let prevouts = psbt
        .inputs
        .iter()
        .map(|i| i.witness_utxo.as_ref().ok_or(CalculateSighashError::MissingWitnessUtxo))
        .collect::<Result<Vec<_>, CalculateSighashError>>()?;
    let sighash = sighashcache.taproot_key_spend_signature_hash(
        input_index,
        &bitcoin::sighash::Prevouts::All(&prevouts),
        TapSighashType::All,
    )?;

    Ok(sighash)
}
