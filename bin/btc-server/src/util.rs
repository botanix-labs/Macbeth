use crate::{database::Utxo, Error, SECP};
use bitcoin::{
    consensus::encode as btcencode,
    hashes::Hash,
    psbt::{self, Psbt},
    OutPoint,
};
use frost_secp256k1_tr as frost;
use reth_btc_wallet::transaction::{
    ETH_ADDRESS_FIELD, PARTIAL_SIGNATURE_KEY_TYPE, SIGNING_COMMITMENTS_KEY_TYPE,
};
use std::{collections::BTreeMap, fmt};

/// Extension trait for OutPoint.
pub trait OutPointExt: Into<OutPoint> {
    fn to_bytes(self) -> [u8; 36] {
        let OutPoint { txid, vout } = self.into();
        let mut ret = [0u8; 36];
        ret[0..32].copy_from_slice(&txid[..]);
        ret[32..].copy_from_slice(&vout.to_le_bytes()[..]);
        ret
    }

    fn from_bytes(b: [u8; 36]) -> OutPoint {
        btcencode::deserialize(&b).expect("always deserializes")
    }

    fn from_slice(b: &[u8]) -> Result<OutPoint, btcencode::Error> {
        btcencode::deserialize(&b)
    }

    // stopgap for dealing with BDK with other rust-bitcoin version
    fn to_bdk(self) -> bdk::bitcoin::OutPoint {
        let OutPoint { txid, vout } = self.into();
        bdk::bitcoin::OutPoint {
            txid: bdk::bitcoin::hashes::Hash::from_slice(&txid.to_byte_array()).unwrap(),
            vout,
        }
    }

    fn from_bdk(outpoint: bdk::bitcoin::OutPoint) -> OutPoint {
        bitcoin::OutPoint { txid: outpoint.txid, vout: outpoint.vout }
    }
}

impl OutPointExt for OutPoint {}

#[derive(Debug, Clone, Error)]
pub enum VerifyingKeyExtError {
    FailedToConvertToSecpPk(bitcoin::secp256k1::Error),
}

impl fmt::Display for VerifyingKeyExtError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            VerifyingKeyExtError::FailedToConvertToSecpPk(err) => {
                write!(f, "Failed to convert to secp pk: {}", err)
            }
        }
    }
}
/// Extension trait for Frost verifying key (aggregate key)
pub trait VerifyingKeyExt: Into<frost::VerifyingKey> {
    fn to_secp_pk(self) -> Result<bitcoin::secp256k1::PublicKey, VerifyingKeyExtError> {
        let vk: frost::VerifyingKey = self.into();
        let pk =
            bitcoin::secp256k1::PublicKey::from_slice(vk.serialize().as_slice()).map_err(|e| {
                log::error!("Failed to convert to secp pk: {}", e);
                VerifyingKeyExtError::FailedToConvertToSecpPk(e)
            })?;

        Ok(pk)
    }
}

impl VerifyingKeyExt for frost::VerifyingKey {}

#[derive(Debug, Error)]
pub enum ParsingError {
    #[error("invalid frost id")]
    InvalidFrostPeerId,
    #[error("invalid signing session id")]
    InvalidSigningSessionId,
    #[error("invalid eth address: {0}")]
    InvalidEthAddress(&'static str),
}

// Deserializes a Frost peer ID.
///
/// # Arguments
///
/// * `id` - The peer ID to be decoded.
///
/// # Returns
///
/// Returns a `Result` containing the serialized Frost identifier if successful, or an `Error` if
/// the peer ID is invalid.
pub fn deserialize_frost_peer_id(id: Vec<u8>) -> Result<frost::Identifier, ParsingError> {
    if id.len() != 32 {
        return Err(ParsingError::InvalidFrostPeerId);
    }
    let peer_id_bytes: &[u8; 32] =
        id.as_slice().try_into().map_err(|_e| ParsingError::InvalidFrostPeerId)?;

    let frost_id = frost::Identifier::deserialize(&peer_id_bytes)
        .map_err(|_e| ParsingError::InvalidFrostPeerId)?;

    Ok(frost_id)
}

/// Parses an Ethereum address string into a byte array.
///
/// # Arguments
///
/// * `eth_address` - The Ethereum address string to be parsed.
///
/// # Returns
///
/// Returns a Result containing the parsed Ethereum address as a fixed-size byte array if
/// successful, or an Error if the parsing fails.
pub fn parse_eth_address(eth_address: String) -> Result<[u8; 20], ParsingError> {
    let eth_address = eth_address.trim_start_matches("0x").to_ascii_lowercase();
    let eth_addr_vec = hex::decode(eth_address)
        .map_err(|_e| ParsingError::InvalidEthAddress("Failed to decode hex"))?;
    if eth_addr_vec.len() != 20 {
        return Err(ParsingError::InvalidEthAddress("Eth address must be 20 bytes"));
    }

    let eth_addr: [u8; 20] = eth_addr_vec
        .try_into()
        .map_err(|_e| ParsingError::InvalidEthAddress("Failed to map eth address to 20 bytes"))?;

    Ok(eth_addr)
}

pub fn parse_signing_session_id(session_id: &Vec<u8>) -> Result<[u8; 32], ParsingError> {
    if session_id.len() != 32 {
        return Err(ParsingError::InvalidSigningSessionId);
    }
    let mut session_id_array = [0u8; 32];
    session_id_array.copy_from_slice(&session_id);
    Ok(session_id_array)
}

/// Adds or removes UTXOs (Unspent Transaction Outputs) from the database based on the given PSBT
/// (Partially Signed Bitcoin Transaction), public key, and associated Bitcoin transaction details.
///
/// # Arguments
///
/// * `psbt` - A reference to the PSBT (Partially Signed Bitcoin Transaction) containing transaction
///   details.
/// * `pk` - A reference to the aggregate secp256k1 public key. This key is NOT tweaked with any
///   taptweaks or eth addresses.
///
/// # Returns
///
/// Returns tuple of two vectors containing the UTXOs added and removed from the database.
pub fn add_remove_utxo_from_psbt(
    psbt: &Psbt,
    pk: &bitcoin::secp256k1::PublicKey,
) -> (Vec<Utxo>, Vec<OutPoint>) {
    let tx = psbt.clone().extract_tx();
    let selected_inputs = tx.input.iter().map(|i| i.previous_output).collect::<Vec<OutPoint>>();
    // For change outputs there will always be a no eth tweak
    let mut change_outputs: Vec<Utxo> = vec![];
    let change_spk = reth_btc_wallet::address::generate_taproot_change_scriptpubkey(&SECP, pk);
    for (index, output) in tx.output.iter().enumerate() {
        if output.script_pubkey == change_spk {
            change_outputs.push(Utxo {
                outpoint: OutPoint::new(tx.txid(), index as u32),
                output: output.clone(),
                eth_address: None,
            });
        }
    }
    (change_outputs, selected_inputs)
}

/// Errors that can occur during the conversion from a PSBT to
/// a vector of signing packages for Frost signature generation.
#[derive(Debug, Error)]
pub enum PsbtToSigningPackageConversionError {
    #[error("Failed to calculate sighash: {0}")]
    FailedToCalculateSighash(#[from] reth_btc_wallet::transaction::CalculateSighashError),
    #[error("Missing signing commitments")]
    MissingSigningCommitments,
    #[error("Failed to deserialize signing commitments")]
    FailedToDeserializeSigningCommitments(#[from] serde_json::Error),
    #[error("Frost error: {0}")]
    FrostError(#[from] frost::Error),
    #[error("Failed to deserialize frost peer id")]
    FailedToDeserializeFrostPeerId(#[from] crate::util::ParsingError),
}

/// Converts a PSBT into a vector of Frost signing packages.
///
/// This function takes a PSBT as input and processes each input to generate the necessary signing
/// packages for Frost signature generation. It returns a vector of `frost::SigningPackage`
/// instances, each containing the signing commitments and other relevant information for the
/// corresponding PSBT input.
///
/// # Arguments
///
/// * `psbt` - A reference to the PSBT to be converted into signing packages.
///
/// # Returns
///
/// Returns a `Result` containing a vector of `frost::SigningPackage` instances if the conversion is
/// successful, or an error of type `PsbtToSigningPackageConversionError` otherwise.
pub fn psbt_to_signing_packages(
    psbt: &Psbt,
) -> Result<Vec<frost::SigningPackage>, PsbtToSigningPackageConversionError> {
    let mut signing_packages = vec![];
    // Retrieve all frost ids from psbt inputs
    for (index, input) in psbt.inputs.iter().enumerate() {
        let sighash = reth_btc_wallet::transaction::calculate_sighash(&psbt, index)?;
        let eth_tweak = input.unknown.get(&ETH_ADDRESS_FIELD.clone());

        // Check if there are any signing commitments
        let mut sc = BTreeMap::new();
        for (k, v) in input.unknown.iter() {
            if k.type_value == SIGNING_COMMITMENTS_KEY_TYPE.clone() {
                let signing_commitments =
                    frost::round1::SigningCommitments::deserialize(v.as_slice())?;
                // First byte encodes key data size which should always be 32
                // Psbt raw keys are structured as keylen keydata keytype
                let frost_id = deserialize_frost_peer_id(k.key.clone()[1..33].to_vec())?;
                sc.insert(frost_id, signing_commitments);
            }
        }

        if sc.is_empty() {
            return Err(PsbtToSigningPackageConversionError::MissingSigningCommitments);
        }

        let mut signing_package =
            frost::SigningPackage::new(sc, sighash.to_raw_hash().to_byte_array().as_slice());
        if let Some(e) = eth_tweak {
            signing_package.set_addtional_tweak(e.clone());
        };

        signing_packages.push(signing_package);
    }
    Ok(signing_packages)
}

// TODO the next four functions are very similar, we should refactor them

pub fn add_signing_commitments_to_psbt(
    psbt: &mut Psbt,
    signing_commitments: &Vec<frost::round1::SigningCommitments>,
    frost_id: &frost::Identifier,
) {
    let frost_id_bytes = frost_id.serialize();
    let mut key: Vec<u8> = vec![frost_id_bytes.len() as u8];
    key.extend(frost_id_bytes);
    key.push(SIGNING_COMMITMENTS_KEY_TYPE.clone());

    let key = psbt::raw::Key { type_value: SIGNING_COMMITMENTS_KEY_TYPE.clone(), key };
    for (_index, (input, sc)) in psbt.inputs.iter_mut().zip(signing_commitments.iter()).enumerate()
    {
        input.unknown.insert(key.clone(), sc.serialize().expect("valid signing commitments"));
    }
}

pub fn add_partial_signature_to_psbt(
    psbt: &mut Psbt,
    partial_signature: &Vec<frost::round2::SignatureShare>,
    frost_id: &frost::Identifier,
) {
    let frost_id_bytes = frost_id.serialize();
    let mut key: Vec<u8> = vec![frost_id_bytes.len() as u8];
    key.extend(frost_id_bytes);
    key.push(PARTIAL_SIGNATURE_KEY_TYPE.clone());

    let key = psbt::raw::Key { type_value: PARTIAL_SIGNATURE_KEY_TYPE.clone(), key };
    for (_index, (input, sig)) in psbt.inputs.iter_mut().zip(partial_signature.iter()).enumerate() {
        input.unknown.insert(key.clone(), sig.serialize().to_vec());
    }
}

#[derive(Debug, Error)]
pub enum RetrieveUnknownKeyError {
    #[error("key was not found in input unknown fields")]
    KeyNotFound,
    #[error("failed to deserialize value")]
    ValueFormatError(#[from] std::array::TryFromSliceError),
    #[error("invalid value for key: {0}")]
    InvalidValue(#[from] frost::Error),
    #[error("frost id parsing error: {0}")]
    FrostIdParsingError(#[from] ParsingError),
}

pub fn retrieve_partial_signatures(
    psbt: &Psbt,
    frost_id: &frost::Identifier,
) -> Result<Vec<frost::round2::SignatureShare>, RetrieveUnknownKeyError> {
    let frost_id_bytes = frost_id.serialize();
    let key_type = PARTIAL_SIGNATURE_KEY_TYPE.clone();
    let mut key: Vec<u8> = vec![frost_id_bytes.len() as u8];
    key.extend(frost_id_bytes);
    key.push(key_type);

    let key = psbt::raw::Key { type_value: key_type, key };

    let mut ret = vec![];
    for input in psbt.inputs.iter() {
        if let Some(value) = input.unknown.get(&key) {
            let partial_sig =
                frost::round2::SignatureShare::deserialize(value.clone().as_slice().try_into()?)?;
            ret.push(partial_sig);
            continue;
        }
        return Err(RetrieveUnknownKeyError::KeyNotFound);
    }
    Ok(ret)
}

pub fn retrieve_all_partial_signatures(
    psbt: &Psbt,
) -> Result<Vec<BTreeMap<frost::Identifier, frost::round2::SignatureShare>>, RetrieveUnknownKeyError>
{
    let key_type = PARTIAL_SIGNATURE_KEY_TYPE.clone();
    let mut ret = vec![];
    for input in psbt.inputs.iter() {
        let mut partial_sigs = BTreeMap::new();
        for (k, v) in input.unknown.iter() {
            if k.type_value == key_type {
                let frost_id = deserialize_frost_peer_id(k.key.clone()[1..33].to_vec())?;
                let partial_sig =
                    frost::round2::SignatureShare::deserialize(v.clone().as_slice().try_into()?)?;
                partial_sigs.insert(frost_id, partial_sig);
            }
        }
        ret.push(partial_sigs);
    }
    Ok(ret)
}

pub fn retrieve_all_signing_commitments(
    psbt: &Psbt,
) -> Result<
    Vec<BTreeMap<frost::Identifier, frost::round1::SigningCommitments>>,
    RetrieveUnknownKeyError,
> {
    let key_type = SIGNING_COMMITMENTS_KEY_TYPE.clone();
    let mut ret = vec![];
    for input in psbt.inputs.iter() {
        let mut signing_commitments = BTreeMap::new();
        for (k, v) in input.unknown.iter() {
            if k.type_value == key_type {
                let frost_id = deserialize_frost_peer_id(k.key.clone()[1..33].to_vec())?;
                let partial_sig =
                    frost::round1::SigningCommitments::deserialize(v.clone().as_slice())?;
                signing_commitments.insert(frost_id, partial_sig);
            }
        }
        ret.push(signing_commitments);
    }
    Ok(ret)
}

pub fn retrieve_signing_commitments(
    psbt: &Psbt,
    frost_id: &frost::Identifier,
) -> Result<Vec<frost::round1::SigningCommitments>, RetrieveUnknownKeyError> {
    let key_type = SIGNING_COMMITMENTS_KEY_TYPE.clone();
    let frost_id_bytes = frost_id.serialize();
    let mut key: Vec<u8> = vec![frost_id_bytes.len() as u8];
    key.extend(frost_id_bytes);
    key.push(key_type);

    let key = psbt::raw::Key { type_value: key_type, key };

    let mut ret = vec![];
    for input in psbt.inputs.iter() {
        if let Some(value) = input.unknown.get(&key) {
            let signing_commitments =
                frost::round1::SigningCommitments::deserialize(value.clone().as_slice())?;
            ret.push(signing_commitments);
            continue;
        }
        return Err(RetrieveUnknownKeyError::KeyNotFound);
    }
    Ok(ret)
}

pub fn convert_bdk_feerate_to_bitcoin(fee_rate: bdk::FeeRate) -> bitcoin::FeeRate {
    bitcoin::FeeRate::from_sat_per_kwu((fee_rate.sat_per_kwu()) as u64)
}

#[cfg(test)]
mod util_tests {
    use bitcoin::{ScriptBuf, TxOut};

    use crate::test::{create_tx, eth_vector_to_fixed_bytes, trusted_dealer_setup};

    use super::*;

    #[test]
    fn convert_bdk_fee_rate() {
        let bdk_fee = bdk::FeeRate::from_sat_per_vb(10.0);
        let rust_bitcoin_fee = bitcoin::FeeRate::from_sat_per_vb(10).unwrap();

        let converted_fee = convert_bdk_feerate_to_bitcoin(bdk_fee);
        assert_eq!(converted_fee, rust_bitcoin_fee);
    }

    #[test]
    fn should_add_signing_commits_to_psbt() {
        let num_inputs = 2;
        let frost_id1 = frost::Identifier::try_from(1u16).expect("valid id");
        let frost_id2 = frost::Identifier::try_from(2u16).expect("valid id");
        let frost_ids = vec![frost_id1, frost_id2];

        let (shares, _pk_package) = trusted_dealer_setup(2, 3);
        let key_package1 = frost::keys::KeyPackage::try_from(
            shares[&frost::Identifier::try_from(1u16).expect("valid id")].clone(),
        )
        .expect("valid key package");

        let key_package2 = frost::keys::KeyPackage::try_from(
            shares[&frost::Identifier::try_from(2u16).expect("valid id")].clone(),
        )
        .expect("valid key package");
        let rng = &mut rand::thread_rng();

        // generate signing commitments for each input for each frost participant
        let (_, signing_commits1_0) = frost::round1::commit(key_package1.signing_share(), rng);
        let (_, signing_commits1_1) = frost::round1::commit(key_package1.signing_share(), rng);

        let (_, signing_commits2_0) = frost::round1::commit(key_package2.signing_share(), rng);
        let (_, signing_commits2_1) = frost::round1::commit(key_package2.signing_share(), rng);

        let scs_input_0 = vec![signing_commits1_0, signing_commits2_0];
        let scs_input_1 = vec![signing_commits1_1, signing_commits2_1];

        let tx = create_tx(num_inputs);

        let mut psbt = Psbt::from_unsigned_tx(tx.clone()).unwrap();
        // Add signing commitments to the psbt for each input
        add_signing_commitments_to_psbt(
            &mut psbt,
            &vec![signing_commits1_0, signing_commits1_1],
            &frost_id1,
        );
        add_signing_commitments_to_psbt(
            &mut psbt,
            &vec![signing_commits2_0, signing_commits2_1],
            &frost_id2,
        );
        for (i, input) in psbt.inputs.iter().enumerate() {
            let key = input.unknown.keys();
            let values = input.unknown.values();
            assert_eq!(key.len(), 2);
            assert_eq!(values.len(), 2);

            for (j, k) in key.enumerate() {
                let k_bytes = k.key.clone();
                assert_eq!(k_bytes[0], 32u8);
                assert_eq!(k_bytes[1..33], frost_ids[j].serialize());
                assert_eq!(k_bytes[33], SIGNING_COMMITMENTS_KEY_TYPE.clone());
            }

            for (j, v) in values.enumerate() {
                let sc = frost::round1::SigningCommitments::deserialize(&v)
                    .expect("valid signing commits");
                assert_eq!(sc, if i == 0 { scs_input_0[j] } else { scs_input_1[j] });
            }
        }
        // lets try to retrieve the signing commitments
        let retrieved_scs = retrieve_signing_commitments(&psbt, &frost_id1).expect("valid scs");
        assert_eq!(retrieved_scs, vec![signing_commits1_0, signing_commits1_1]);

        let retrieved_scs = retrieve_signing_commitments(&psbt, &frost_id2).expect("valid scs");
        assert_eq!(retrieved_scs, vec![signing_commits2_0, signing_commits2_1]);
    }

    #[test]
    fn should_add_partial_signatures_to_psbt() {
        let num_inputs = 2;
        let frost_id1 = frost::Identifier::try_from(1u16).expect("valid id");
        let frost_id2 = frost::Identifier::try_from(2u16).expect("valid id");
        let frost_ids = vec![frost_id1, frost_id2];

        let sig_share1_0 =
            frost::round2::SignatureShare::deserialize([1u8; 32]).expect("valid sig share");
        let sig_share1_1 =
            frost::round2::SignatureShare::deserialize([2u8; 32]).expect("valid sig share");

        let sig_share2_0 =
            frost::round2::SignatureShare::deserialize([3u8; 32]).expect("valid sig share");
        let sig_share2_1 =
            frost::round2::SignatureShare::deserialize([4u8; 32]).expect("valid sig share");

        let partial_sigs_input_0 = vec![sig_share1_0, sig_share2_0];
        let partial_sigs_input_1 = vec![sig_share1_1, sig_share2_1];

        let tx = create_tx(num_inputs);
        let mut psbt = Psbt::from_unsigned_tx(tx.clone()).unwrap();
        // Add signing commitments to the psbt for each input
        add_partial_signature_to_psbt(&mut psbt, &vec![sig_share1_0, sig_share1_1], &frost_id1);
        add_partial_signature_to_psbt(&mut psbt, &vec![sig_share2_0, sig_share2_1], &frost_id2);

        for (i, input) in psbt.inputs.iter().enumerate() {
            let key = input.unknown.keys();
            let values = input.unknown.values();
            assert_eq!(key.len(), 2);
            assert_eq!(values.len(), 2);

            for (j, k) in key.enumerate() {
                let k_bytes = k.key.clone();
                assert_eq!(k_bytes[0], 32u8);
                assert_eq!(k_bytes[1..33], frost_ids[j].serialize());
                assert_eq!(k_bytes[33], PARTIAL_SIGNATURE_KEY_TYPE.clone());
            }

            for (j, v) in values.enumerate() {
                let fixed_size_bytes: [u8; 32] = v.as_slice().try_into().unwrap();
                let sc = frost::round2::SignatureShare::deserialize(fixed_size_bytes)
                    .expect("valid signing commits");
                assert_eq!(
                    sc,
                    if i == 0 { partial_sigs_input_0[j] } else { partial_sigs_input_1[j] }
                );
            }
        }

        // lets try to retrieve
        let retrieved_sigs = retrieve_partial_signatures(&psbt, &frost_id1).expect("valid sigs");
        assert_eq!(retrieved_sigs, vec![sig_share1_0, sig_share1_1]);

        let retrieved_sigs = retrieve_partial_signatures(&psbt, &frost_id2).expect("valid sigs");
        assert_eq!(retrieved_sigs, vec![sig_share2_0, sig_share2_1]);
    }

    #[test]
    fn signing_package_conversion_should_fail_when_missing_signing_commitments() {
        let tx = create_tx(1);
        let mut psbt = Psbt::from_unsigned_tx(tx.clone()).unwrap();
        psbt.inputs[0].witness_utxo = Some(TxOut { value: 1000, script_pubkey: ScriptBuf::new() });

        let signing_packages = psbt_to_signing_packages(&psbt);
        println!("{:?}", signing_packages);
        assert!(signing_packages.is_err());
        assert_eq!(
            signing_packages.unwrap_err().to_string(),
            PsbtToSigningPackageConversionError::MissingSigningCommitments.to_string()
        );
    }

    #[test]
    fn should_generate_singning_packages() {
        // Setup
        let num_inputs = 2;
        let frost_id1 = frost::Identifier::try_from(1u16).expect("valid id");
        let frost_id2 = frost::Identifier::try_from(2u16).expect("valid id");

        let (shares, _pk_package) = trusted_dealer_setup(2, 3);
        let key_package1 = frost::keys::KeyPackage::try_from(shares[&frost_id1].clone())
            .expect("valid key package");
        let key_package2 = frost::keys::KeyPackage::try_from(shares[&frost_id2].clone())
            .expect("valid key package");

        let rng = &mut rand::thread_rng();

        // Get some signing commitments
        let (_, signing_commits1_0) = frost::round1::commit(key_package1.signing_share(), rng);
        let (_, signing_commits1_1) = frost::round1::commit(key_package1.signing_share(), rng);

        let (_, signing_commits2_0) = frost::round1::commit(key_package2.signing_share(), rng);
        let (_, signing_commits2_1) = frost::round1::commit(key_package2.signing_share(), rng);

        // Set up the psbt
        let tx = create_tx(num_inputs);
        let mut psbt = Psbt::from_unsigned_tx(tx.clone()).unwrap();
        // Add signing commitments and TxOut to the psbt for each input
        psbt.inputs[0].witness_utxo = Some(TxOut { value: 1000, script_pubkey: ScriptBuf::new() });
        psbt.inputs[1].witness_utxo = Some(TxOut { value: 1000, script_pubkey: ScriptBuf::new() });
        add_signing_commitments_to_psbt(
            &mut psbt,
            &vec![signing_commits1_0, signing_commits1_1],
            &frost_id1,
        );
        add_signing_commitments_to_psbt(
            &mut psbt,
            &vec![signing_commits2_0, signing_commits2_1],
            &frost_id2,
        );

        // Add a eth tweak to the first input
        let eth_tweak = eth_vector_to_fixed_bytes(vec![1u8; 20]);
        psbt.inputs[0].unknown.insert(ETH_ADDRESS_FIELD.clone(), eth_tweak.to_vec());

        let signing_packages = psbt_to_signing_packages(&psbt).expect("valid list signing package");
        assert_eq!(signing_packages.len(), 2);
        assert_eq!(
            signing_packages[0]
                .signing_commitments()
                .values()
                .map(|sc| sc.clone())
                .collect::<Vec<frost::round1::SigningCommitments>>()
                .clone(),
            vec![signing_commits1_0.clone(), signing_commits2_0.clone()]
        );
        // check the frost ids as well
        assert_eq!(
            signing_packages[0]
                .signing_commitments()
                .keys()
                .map(|f| f.clone())
                .collect::<Vec<frost::Identifier>>()
                .clone(),
            vec![frost_id1.clone(), frost_id2.clone()]
        );
        assert!(signing_packages[0].additional_tweak().as_ref().unwrap() == &eth_tweak.to_vec());

        assert_eq!(
            signing_packages[1]
                .signing_commitments()
                .values()
                .map(|sc| sc.clone())
                .collect::<Vec<frost::round1::SigningCommitments>>()
                .clone(),
            vec![signing_commits1_1.clone(), signing_commits2_1.clone()]
        );
        // check the frost ids as well
        assert_eq!(
            signing_packages[1]
                .signing_commitments()
                .keys()
                .map(|f| f.clone())
                .collect::<Vec<frost::Identifier>>()
                .clone(),
            vec![frost_id1.clone(), frost_id2.clone()]
        );
        assert!(signing_packages[1].additional_tweak().is_none());
    }

    #[test]
    fn test_deserialize_frost_peer_id() {
        // Valid peer ID, len = 32
        let valid_id: Vec<u8> = vec![
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
            0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C,
            0x1D, 0x1E, 0x1F, 0x20,
        ];
        let result = deserialize_frost_peer_id(valid_id);
        assert!(result.is_ok());
        result.unwrap();

        // Invalid peer ID (length is not 32)
        let invalid_id: Vec<u8> = vec![0x01, 0x02, 0x03];
        let result = deserialize_frost_peer_id(invalid_id);
        assert!(result.is_err());

        // encode and decode the id 0
        let peer_id0 = 0u16;
        let f = frost::Identifier::derive(&peer_id0.to_be_bytes().to_vec()).unwrap();
        let f_bytes = f.serialize().to_vec();
        let peer_id_decoded = deserialize_frost_peer_id(f_bytes.to_vec()).unwrap();

        assert_eq!(f, peer_id_decoded);
    }

    #[test]
    fn test_parse_eth_address() {
        // Valid Ethereum address
        let valid_eth_address = "0123456789abcdef0123456789abcdef01234567".to_string();
        let result = parse_eth_address(valid_eth_address);
        assert!(result.is_ok());
        let parsed_address = result.unwrap();
        assert_eq!(
            parsed_address,
            [
                1, 35, 69, 103, 137, 171, 205, 239, 1, 35, 69, 103, 137, 171, 205, 239, 1, 35, 69,
                103
            ]
        );

        // Should stip 0x prefix
        let valid_eth_address = "0x0123456789abcdef0123456789abcdef01234567".to_string();
        let result = parse_eth_address(valid_eth_address);
        assert!(result.is_ok());
        let parsed_address = result.unwrap();
        assert_eq!(
            parsed_address,
            [
                1, 35, 69, 103, 137, 171, 205, 239, 1, 35, 69, 103, 137, 171, 205, 239, 1, 35, 69,
                103
            ]
        );

        // Invalid Ethereum address (not enough bytes)
        let invalid_eth_address = "0123456789abcdef01234567".to_string();
        let result = parse_eth_address(invalid_eth_address);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            ParsingError::InvalidEthAddress("Eth address must be 20 bytes").to_string()
        );

        // Invalid Ethereum address (failed to decode hex)
        let invalid_eth_address = "0123456789abcdef0123456789abcdef0123456g".to_string();
        let result = parse_eth_address(invalid_eth_address);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            ParsingError::InvalidEthAddress("Failed to decode hex").to_string()
        );
    }
}
