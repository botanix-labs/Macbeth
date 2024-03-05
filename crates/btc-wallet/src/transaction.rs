use bitcoin::{
    psbt::{self, PartiallySignedTransaction, Psbt},
    sighash::{TapSighash, TapSighashType},
    OutPoint, TxOut,
};

lazy_static::lazy_static!(
    pub static ref ETH_ADDRESS_FIELD: psbt::raw::Key = psbt::raw::Key { type_value: 0xff, key: vec![0, 0xff] };
    pub static ref SIGNING_COMMITMENTS_KEY_TYPE: u8 = 0xde;
    pub static ref PARTIAL_SIGNATURE_KEY_TYPE: u8 = 0xdf;
    pub static ref SIGNING_COMMITMENTS: psbt::raw::Key = psbt::raw::Key { type_value: 0xde, key: vec![0, 0xde] };
);

/// Utxo DTO struct
pub struct Input {
    pub outpoint: OutPoint,
    pub output: TxOut,
    pub eth_address: Option<[u8; 20]>,
}

/// Create psbt with proprietary tweak fields
pub fn create_psbt(inputs: Vec<Input>, outputs: Vec<TxOut>, change: Option<TxOut>) -> Psbt {
    let tx = bitcoin::Transaction {
        version: 2i32,
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
        output: {
            let mut ret = outputs;
            if let Some(change) = change.clone() {
                ret.push(change);
            }
            ret
        },
    };

    // Create PSBT
    let mut psbt = PartiallySignedTransaction::from_unsigned_tx(tx).expect("tx is unsigned");
    for (psbt_input, utxo) in psbt.inputs.iter_mut().zip(inputs.iter()) {
        psbt_input.witness_utxo = Some(utxo.output.clone());
        if utxo.eth_address.is_some() {
            // Key stores no keydata, only the type value
            psbt_input.unknown.insert(
                ETH_ADDRESS_FIELD.clone(),
                utxo.eth_address.expect("have eth address").to_vec(),
            );
        }
    }

    psbt
}

#[derive(Debug, thiserror::Error)]
pub enum CalculateSighashError {
    #[error("Failed to calculate sighash: {0}")]
    FailedToCalculateSighash(#[from] bitcoin::sighash::Error),
}

/// Calculate the sighash for a taproot keyspend
/// Using tapsighash type ALL
pub fn calculate_sighash(
    psbt: &Psbt,
    input_index: usize,
) -> Result<TapSighash, CalculateSighashError> {
    let mut sighashcache = bitcoin::sighash::SighashCache::new(&psbt.unsigned_tx);

    // TODO(armins) remove unwrap
    let prevouts = psbt.inputs.iter().map(|i| i.witness_utxo.as_ref().unwrap()).collect::<Vec<_>>();
    let sighash = sighashcache.taproot_key_spend_signature_hash(
        input_index,
        &psbt::Prevouts::All(&prevouts),
        TapSighashType::All,
    )?;

    Ok(sighash)
}
