use bitcoin::{
    psbt::{self, PartiallySignedTransaction, Psbt},
    sighash::{TapSighash, TapSighashType},
    OutPoint, TxOut,
};

const USER_ETH_ADDRESS_FIELD: u8 = 1;

static ETH_ADDRESS_FIELD: psbt::raw::ProprietaryKey = psbt::raw::ProprietaryKey {
    prefix: Vec::new(),
    subtype: USER_ETH_ADDRESS_FIELD,
    key: Vec::new(),
};

/// Utxo DTO struct
pub struct Input {
    pub outpoint: OutPoint,
    pub output: TxOut,
    pub eth_address: Option<[u8; 20]>,
}

/// Create psbt with proprietary tweak fields
pub fn create_psbt(
    inputs: Vec<Input>,
    outputs: Vec<TxOut>,
    change: Option<TxOut>,
) -> PartiallySignedTransaction {
    let tx = bitcoin::Transaction {
        version: 2,
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
    for (psbt, utxo) in psbt.inputs.iter_mut().zip(inputs.iter()) {
        psbt.witness_utxo = Some(utxo.output.clone());
        // store the user tweak if used
        if utxo.eth_address.is_some() {
            psbt.proprietary.insert(
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
pub fn calculate_sighash(psbt: &Psbt) -> Result<TapSighash, CalculateSighashError> {
    let mut sighashcache = bitcoin::sighash::SighashCache::new(&psbt.unsigned_tx);

    let prevouts = psbt.inputs.iter().map(|i| i.witness_utxo.as_ref().unwrap()).collect::<Vec<_>>();
    let sighash = sighashcache.taproot_signature_hash(
        0,
        &psbt::Prevouts::All(&prevouts),
        None, // annex
        None, // leaf_hash_code_separator
        TapSighashType::All,
    )?;

    Ok(sighash)
}
