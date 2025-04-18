use std::{borrow::BorrowMut, collections::BTreeMap};

use bitcoin::{
    psbt::{raw::ProprietaryKey, Input as PsbtInput, Output as PsbtOutput, Psbt},
    Amount, OutPoint, TapSighash, TapSighashType, TxOut,
};
use bitcoin_hashes::Hash;
use frost_secp256k1_tr as frost;
use thiserror::Error;

use crate::database::version::UtxoVersion;

// input keys
const ETH_ADDRESS_KEY_TYPE: u8 = 1;
const SIGNING_COMMITMENTS_KEY_TYPE: u8 = 2;
const PARTIAL_SIGNATURE_KEY_TYPE: u8 = 3;
const UTXO_VERSION_TYPE: u8 = 4;

// output keys
const PEGOUT_ID_KEY_TYPE: u8 = 4;

/// eth address tweak
pub type EthAddress = [u8; 20];

lazy_static::lazy_static! {
    static ref PROP_KEY_PREFIX: &'static [u8] = b"btx";

    static ref ETH_ADDRESS_KEY: ProprietaryKey = ProprietaryKey {
        prefix: PROP_KEY_PREFIX.to_vec(),
        subtype: ETH_ADDRESS_KEY_TYPE,
        key: Vec::new(),
    };

    static ref PEGOUT_ID_KEY: ProprietaryKey = ProprietaryKey {
        prefix: PROP_KEY_PREFIX.to_vec(),
        subtype: PEGOUT_ID_KEY_TYPE,
        key: Vec::new(),
    };

    pub static ref UTXO_VERSION_TYPE_KEY: ProprietaryKey = ProprietaryKey {
        prefix: PROP_KEY_PREFIX.to_vec(),
        subtype: UTXO_VERSION_TYPE,
        key: Vec::new(),
    };
}

trait ProprietaryKeyExt: BorrowMut<ProprietaryKey> {
    fn cast(&self, subtype: u8) -> Option<&[u8]> {
        let key = self.borrow();
        if key.prefix == *PROP_KEY_PREFIX && key.subtype == subtype {
            Some(&key.key)
        } else {
            None
        }
    }
}
impl ProprietaryKeyExt for ProprietaryKey {}

pub trait PsbtInputExt: BorrowMut<PsbtInput> {
    fn set_eth_address(&mut self, eth_address: EthAddress) {
        // Key stores no keydata, only the type value
        self.borrow_mut().proprietary.insert(ETH_ADDRESS_KEY.clone(), eth_address.to_vec());
    }

    /// Adds version information to PSBT inputs
    fn add_version_to_psbt(&mut self, version: u32) {
        self.borrow_mut()
            .proprietary
            .insert(UTXO_VERSION_TYPE_KEY.clone(), (version).to_le_bytes().to_vec());
    }

    /// Gets the version of a UTXO from a PSBT input
    fn get_version_from_psbt_input(&self) -> Option<UtxoVersion> {
        self.borrow().proprietary.get(&UTXO_VERSION_TYPE_KEY).and_then(|bytes| {
            if bytes.len() == 4 {
                let version = u32::from_le_bytes(bytes.as_slice().try_into().ok()?);
                UtxoVersion::try_from(version).ok()
            } else {
                None
            }
        })
    }

    fn eth_address(&self) -> Option<EthAddress> {
        self.borrow().proprietary.get(&ETH_ADDRESS_KEY).and_then(|b| {
            if b.len() == 20 {
                let mut ret = [0u8; 20];
                ret.copy_from_slice(&b[..]);
                Some(ret)
            } else {
                None
            }
        })
    }

    /// Set the signing commitment for this input.
    fn set_signing_commitment(
        &mut self,
        frost_id: frost::Identifier,
        commit: &frost::round1::SigningCommitments,
    ) {
        let key = ProprietaryKey {
            prefix: PROP_KEY_PREFIX.to_vec(),
            subtype: SIGNING_COMMITMENTS_KEY_TYPE,
            key: frost_id.serialize().to_vec(),
        };
        let payload = commit.serialize().expect("commit serialize can fail??");
        self.borrow_mut().proprietary.insert(key, payload);
    }

    /// Get the signing commitment for the given frost id from this inputs.
    fn signing_commitments(
        &self,
        frost_id: frost::Identifier,
    ) -> Option<frost::round1::SigningCommitments> {
        let key = ProprietaryKey {
            prefix: PROP_KEY_PREFIX.to_vec(),
            subtype: SIGNING_COMMITMENTS_KEY_TYPE,
            key: frost_id.serialize().to_vec(),
        };
        if let Some(b) = self.borrow().proprietary.get(&key) {
            Some(frost::round1::SigningCommitments::deserialize(b).ok()?)
        } else {
            None
        }
    }

    /// Get all the signing commitment from this inputs for all frost ids.
    fn all_signing_commitments(
        &self,
    ) -> BTreeMap<frost::Identifier, frost::round1::SigningCommitments> {
        let mut ret = BTreeMap::new();
        for (key, value) in self.borrow().proprietary.iter() {
            if let Some(key) = key.cast(SIGNING_COMMITMENTS_KEY_TYPE) {
                let frost_id = match frost_id_from_bytes(key) {
                    Some(v) => v,
                    None => continue,
                };
                let sc = match frost::round1::SigningCommitments::deserialize(value) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                ret.insert(frost_id, sc);
            }
        }
        ret
    }

    /// Set the partial signature for this input.
    fn set_partial_signature(
        &mut self,
        frost_id: frost::Identifier,
        sig: &frost::round2::SignatureShare,
    ) {
        let key = ProprietaryKey {
            prefix: PROP_KEY_PREFIX.to_vec(),
            subtype: PARTIAL_SIGNATURE_KEY_TYPE,
            key: frost_id.serialize().to_vec(),
        };
        let payload = sig.serialize().to_vec();
        self.borrow_mut().proprietary.insert(key, payload);
    }

    /// Get the partial signature for the given frost id from this inputs.
    fn partial_signature(
        &self,
        frost_id: frost::Identifier,
    ) -> Option<frost::round2::SignatureShare> {
        let key = ProprietaryKey {
            prefix: PROP_KEY_PREFIX.to_vec(),
            subtype: PARTIAL_SIGNATURE_KEY_TYPE,
            key: frost_id.serialize().to_vec(),
        };
        if let Some(b) = self.borrow().proprietary.get(&key) {
            Some(signature_share_from_bytes(b)?)
        } else {
            None
        }
    }

    /// Get all the partial signatures from this inputs for all frost ids.
    fn all_partial_signatures(&self) -> BTreeMap<frost::Identifier, frost::round2::SignatureShare> {
        let mut ret = BTreeMap::new();
        for (key, value) in self.borrow().proprietary.iter() {
            if let Some(key) = key.cast(PARTIAL_SIGNATURE_KEY_TYPE) {
                let frost_id = match frost_id_from_bytes(key) {
                    Some(v) => v,
                    None => continue,
                };
                let sc = match signature_share_from_bytes(value) {
                    Some(v) => v,
                    None => continue,
                };
                ret.insert(frost_id, sc);
            }
        }
        ret
    }
}
impl PsbtInputExt for PsbtInput {}

pub type PegoutId = [u8; 36];

pub trait PsbtOutputExt: BorrowMut<PsbtOutput> {
    fn set_pegout_id(&mut self, pegout_id: PegoutId) {
        // Key stores no keydata, only the type value
        self.borrow_mut().proprietary.insert(PEGOUT_ID_KEY.clone(), pegout_id.to_vec());
    }

    fn pegout_id(&self) -> Option<PegoutId> {
        self.borrow().proprietary.get(&PEGOUT_ID_KEY).and_then(|b| {
            if b.len() == 36 {
                let mut ret = [0u8; 36];
                ret.copy_from_slice(&b[..]);
                Some(ret)
            } else {
                None
            }
        })
    }
}
impl PsbtOutputExt for PsbtOutput {}

pub trait PsbtExt: BorrowMut<Psbt> {
    /// Get all pegouts ids from this PSBT
    fn pegout_ids(&self) -> Vec<PegoutId> {
        self.borrow().outputs.iter().filter_map(|o| o.pegout_id()).collect()
    }

    /// Converts this PSBT into a vector of Frost signing packages.
    ///
    /// This function takes a PSBT as input and processes each input to generate the necessary
    /// signing packages for Frost signature generation. It returns a vector of
    /// `frost::SigningPackage` instances, each containing the signing commitments and other
    /// relevant information for the corresponding PSBT input.
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing a vector of `frost::SigningPackage` instances if the
    /// conversion is successful, or an error of type `PsbtToSigningPackageConversionError`
    /// otherwise.
    fn signing_packages(
        &self,
    ) -> Result<Vec<frost::SigningPackage>, PsbtToSigningPackageConversionError> {
        let mut ret = Vec::new();
        for (idx, input) in self.borrow().inputs.iter().enumerate() {
            let sighash = calculate_sighash(self.borrow(), idx)?;

            // Check if there are any signing commitments
            let sc = input.all_signing_commitments();
            if sc.is_empty() {
                return Err(PsbtToSigningPackageConversionError::MissingSigningCommitments);
            }

            let signing_package =
                frost::SigningPackage::new(sc, sighash.to_raw_hash().as_byte_array().as_slice());
            ret.push(signing_package);
        }
        Ok(ret)
    }

    /// Get the fee per output for this PSBT.
    /// Self only needs to be mutable so it can be included in this trait.
    fn fee_per_output(&self, num_outputs: u64) -> Result<Amount, PsbtFeePerOutputError> {
        // calculate fee per output which is shared across all outputs
        let psbt: &Psbt = self.borrow();
        let fee: Amount = psbt.fee()?;
        let fee_per_output: Amount =
            fee.checked_div(num_outputs).ok_or(PsbtFeePerOutputError::DivideByZero)?;
        Ok(fee_per_output)
    }
}
impl PsbtExt for Psbt {}

/// Errors that can occur when calculating the fee per output for a PSBT.
#[derive(Debug, thiserror::Error)]
pub enum PsbtFeePerOutputError {
    #[error("Failed to calculate fee: {0}")]
    FeeError(#[from] bitcoin::psbt::Error),
    #[error("Division by zero error")]
    DivideByZero,
}

/// Errors that can occur during the conversion from a PSBT to
/// a vector of signing packages for Frost signature generation and aggregation.
#[derive(Debug, Error)]
pub enum PsbtToSigningPackageConversionError {
    #[error("Sighash error: {0}")]
    SighashError(#[from] CalculateSighashError),
    #[error("Missing signing commitments")]
    MissingSigningCommitments,
    #[error("Frost error: {0}")]
    FrostError(#[from] frost::Error),
    #[error("Failed to deserialize frost peer id")]
    FailedToDeserializeFrostPeerId(#[from] crate::wallet::util::ParsingError),
}

pub fn frost_id_from_bytes(b: &[u8]) -> Option<frost::Identifier> {
    frost::Identifier::deserialize(b).ok()
}

pub fn signature_share_from_bytes(b: &[u8]) -> Option<frost::round2::SignatureShare> {
    frost::round2::SignatureShare::deserialize(b).ok()
}

/// Utxo DTO struct
#[derive(Debug, Clone)]
pub(crate) struct InputDTO {
    pub outpoint: OutPoint,
    pub output: TxOut,
    pub eth_address: Option<[u8; 20]>,
    pub version: UtxoVersion,
}

/// Create psbt with proprietary tweak fields
pub(crate) fn create_psbt(
    inputs: Vec<InputDTO>,
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
        psbt_input.add_version_to_psbt(utxo.version as u32);
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
    #[error("taproot error: {0}")]
    TaprootError(#[from] bitcoin::sighash::TaprootError),
    #[error("Missing witness utxo")]
    MissingWitnessUtxo,
}

/// Calculate the sighash for a taproot keyspend
/// Using tapsighash type ALL
pub(crate) fn calculate_sighash(
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
