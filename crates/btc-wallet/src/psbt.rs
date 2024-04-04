use std::{borrow::BorrowMut, collections::BTreeMap};

use bitcoin::{
    hashes::Hash,
    psbt::{raw::ProprietaryKey, Input, Psbt},
};
use frost_secp256k1_tr as frost;

const ETH_ADDRESS_KEY_TYPE: u8 = 1;
const SIGNING_COMMITMENTS_KEY_TYPE: u8 = 2;
const PARTIAL_SIGNATURE_KEY_TYPE: u8 = 3;

lazy_static::lazy_static! {
    static ref PROP_KEY_PREFIX: &'static [u8] = b"btx";

    static ref ETH_ADDRESS_KEY: ProprietaryKey = ProprietaryKey {
        prefix: PROP_KEY_PREFIX.to_vec(),
        subtype: ETH_ADDRESS_KEY_TYPE,
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

pub trait PsbtInputExt: BorrowMut<Input> {
    fn set_eth_address(&mut self, eth_address: [u8; 20]) {
        // Key stores no keydata, only the type value
        self.borrow_mut().proprietary.insert(ETH_ADDRESS_KEY.clone(), eth_address.to_vec());
    }

    fn eth_address(&self) -> Option<[u8; 20]> {
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
            Some(frost::round1::SigningCommitments::deserialize(&b).ok()?)
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
                let sc = match frost::round1::SigningCommitments::deserialize(&value) {
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
            Some(signature_share_from_bytes(&b)?)
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
                let sc = match signature_share_from_bytes(&value) {
                    Some(v) => v,
                    None => continue,
                };
                ret.insert(frost_id, sc);
            }
        }
        ret
    }
}
impl PsbtInputExt for Input {}

pub trait PsbtExt: BorrowMut<Psbt> {
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
            let sighash = crate::transaction::calculate_sighash(self.borrow(), idx)?;

            // Check if there are any signing commitments
            let sc = input.all_signing_commitments();
            if sc.is_empty() {
                return Err(PsbtToSigningPackageConversionError::MissingSigningCommitments);
            }

            let mut signing_package =
                frost::SigningPackage::new(sc, sighash.to_raw_hash().to_byte_array().as_slice());
            if let Some(e) = input.eth_address() {
                signing_package.set_addtional_tweak(e.to_vec());
            };

            ret.push(signing_package);
        }
        Ok(ret)
    }
}
impl PsbtExt for Psbt {}

/// Errors that can occur during the conversion from a PSBT to
/// a vector of signing packages for Frost signature generation.
#[derive(Debug, Error)]
pub enum PsbtToSigningPackageConversionError {
    #[error("Failed to calculate sighash: {0}")]
    FailedToCalculateSighash(#[from] crate::transaction::CalculateSighashError),
    #[error("Missing signing commitments")]
    MissingSigningCommitments,
    #[error("Frost error: {0}")]
    FrostError(#[from] frost::Error),
    #[error("Failed to deserialize frost peer id")]
    FailedToDeserializeFrostPeerId(#[from] crate::util::ParsingError),
}

pub fn frost_id_from_bytes(b: &[u8]) -> Option<frost::Identifier> {
    frost::Identifier::deserialize(&b.try_into().ok()?).ok()
}

pub fn signature_share_from_bytes(b: &[u8]) -> Option<frost::round2::SignatureShare> {
    frost::round2::SignatureShare::deserialize(b.try_into().ok()?).ok()
}
