use std::fmt;

use bitcoin::{
    consensus::encode as btcencode,
    hashes::Hash,
    psbt::{ExtractTxError, Psbt},
    Amount, OutPoint,
};
use frost_secp256k1_tr as frost;
use lazy_static::lazy_static;
use reth_btc_wallet::psbt::PsbtInputExt;

use crate::{database, Error};

lazy_static! {
    // TODO get a fee max amount
    static ref MAX_AMOUNT: bitcoin::Amount = bitcoin::Amount::from_sat(21_000_000 * 100_000_000);
    static ref MAX_FEERATE: bitcoin::FeeRate = bitcoin::FeeRate::from_sat_per_vb(300).expect("valid feerate");
}

// Psbt validation flags
pub const NO_FLAGS: u8 = 0u8;
pub const ROUND1: u8 = 1u8;
pub const ROUND1_TRANSITION: u8 = 1u8 << 1 | ROUND1;
pub const ROUND2: u8 = 1u8 << 2 | ROUND1_TRANSITION;
pub const ROUND2_TRANSITION: u8 = 1u8 << 3 | ROUND1_TRANSITION;

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
        btcencode::deserialize(b)
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

    let frost_id = frost::Identifier::deserialize(peer_id_bytes)
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

pub fn parse_signing_session_id(session_id: &[u8]) -> Result<[u8; 32], ParsingError> {
    if session_id.len() != 32 {
        return Err(ParsingError::InvalidSigningSessionId);
    }
    let mut session_id_array = [0u8; 32];
    session_id_array.copy_from_slice(session_id);
    Ok(session_id_array)
}

#[derive(Debug, Error)]
pub enum ValidatePSBTError {
    #[error("inputs cannot be 0")]
    NoInputs,
    #[error("outputs cannot be 0")]
    NoOutputs,
    #[error("cannot calculate fee")]
    FeeCalculationError(bitcoin::psbt::Error),
    #[error("cannot calculate fee rate")]
    FeeRateCalculationError(),
    #[error("failed fee sanity check")]
    FeeSanityCheck(&'static str),
    #[error("missing witness utxo")]
    MissingWitnessUtxo,
    #[error("cannot find UTXO in db")]
    UtxoNotFound,
    #[error("invalid number of signing commitments")]
    InvalidNumberOfSigningCommitments,
    #[error("invalid number of partial signatures")]
    InvalidNumberOfPartialSignatures,
    #[error("frost id mismatch")]
    FrostIdMismatch,
    #[error("eth tweak mismatch")]
    EthTweakMismatch,
    #[error("txout mismatch")]
    TxOutMismatch,
    #[error("extract tx error: {0}")]
    ExtractTxError(#[from] ExtractTxError),
}

/// Validates PSBT structure and content at a given state in the signing session
///
/// # Arguments
///
/// * `psbt` - The PSBT to be validated.
/// * `flags` - Flags indicating the validation criteria and actions to be performed.
/// * `min_signers` - The minimum number of signers required for certain validations.
/// * `db` - Database reference used for UTXO lookups.
///
/// `NO_FLAGS`: Performs basic sanity checks only.
/// `ROUND1`: Validates witnes_UTXO and UTXO existence in the database. Also checks the validity of
/// the input material. `ROUND1_TRANSITION`: Validates signing commitments in round 1, ensuring the
/// required signers. `ROUND2`: Checks if there are enough round 2 partial signatures. Ensuring we
/// never add more than a quorum of signers `ROUND2_TRANSITION`: Validates partial signatures during
/// the transition to round 2, ensuring signers match and Frost IDs align.
pub fn validate_psbt(
    psbt: &Psbt,
    flags: u8,
    min_signers: u16,
    db: &database::Db,
) -> Result<(), ValidatePSBTError> {
    // Sanity check for # of inputs and outputs
    if psbt.inputs.is_empty() {
        return Err(ValidatePSBTError::NoInputs);
    }
    if psbt.outputs.is_empty() {
        return Err(ValidatePSBTError::NoOutputs);
    }
    // Sanity fee checks
    let fee = psbt.fee().map_err(ValidatePSBTError::FeeCalculationError)?;
    if fee < Amount::ZERO {
        return Err(ValidatePSBTError::FeeSanityCheck("Fee cannot be negative"));
    }
    if fee > *MAX_AMOUNT {
        return Err(ValidatePSBTError::FeeSanityCheck("Fee cannot be greater than max amount"));
    }

    // If we are just validating sanity checks we can stop here
    if flags == NO_FLAGS {
        return Ok(());
    }

    // validate signing commitments in round 1
    let scs = psbt.inputs.iter().map(|i| i.all_signing_commitments()).collect::<Vec<_>>();
    if flags & ROUND1_TRANSITION == ROUND1_TRANSITION {
        if scs.len() != psbt.inputs.len() {
            return Err(ValidatePSBTError::InvalidNumberOfSigningCommitments);
        }
        // Each map should have atleast min_signers number of signing commitments
        for sc in &scs {
            if sc.len() < min_signers as usize {
                return Err(ValidatePSBTError::InvalidNumberOfSigningCommitments);
            }
        }
    }

    // Check if we have enough round 2 partial sigs
    // TODO is this neccecary? Will signing fail?
    let sigs = psbt.inputs.iter().map(|i| i.all_partial_signatures()).collect::<Vec<_>>();
    if flags & ROUND2 == ROUND2 {
        // if any of the maps have min signers we should fail
        for sig in sigs.iter() {
            if sig.len() > min_signers as usize {
                return Err(ValidatePSBTError::InvalidNumberOfPartialSignatures);
            }
        }
    }

    // validate partial sigs in round 2
    if flags & ROUND2_TRANSITION == ROUND2_TRANSITION {
        if sigs.len() != psbt.inputs.len() {
            return Err(ValidatePSBTError::InvalidNumberOfPartialSignatures);
        }
        // Each map should have at least min_signers number of partial sigs
        for sig in sigs.iter() {
            if sig.len() != min_signers as usize {
                return Err(ValidatePSBTError::InvalidNumberOfPartialSignatures);
            }
        }

        // Additionally we should check that the same set of signers provided partial sigs as in
        // round 1
        for (sc, sig) in scs.iter().zip(sigs.iter()) {
            if sc.keys().ne(sig.keys()) {
                return Err(ValidatePSBTError::FrostIdMismatch);
            }
        }
        // Lastly the signers should ensure they are infact in the signing group before providing
        // partial sigs That should be done outside the context of this function
    }

    let tx = psbt.clone().extract_tx()?;
    for (index, psbt_input) in psbt.inputs.iter().enumerate() {
        if flags & ROUND1 == ROUND1 {
            // validate utxo exists in DB
            let outpoint = tx.input[index].previous_output;
            let utxo = db.get_utxo(outpoint).expect("valid utxo");
            if utxo.is_none() {
                return Err(ValidatePSBTError::UtxoNotFound);
            }
            // If the utxo has a eth tweak check the right one is presented in the psbt
            let eth_tweak = utxo.clone().expect("valid utxo").eth_address;
            if let Some(e) = eth_tweak {
                let eth_input_tweak = psbt_input.eth_address();
                if eth_input_tweak != Some(e) {
                    return Err(ValidatePSBTError::EthTweakMismatch);
                }
            }
            // Validate prev out is provided
            if psbt_input.witness_utxo.is_none() {
                return Err(ValidatePSBTError::MissingWitnessUtxo);
            }
            let txout = psbt_input.witness_utxo.as_ref().expect("valid witness utxo");
            // Check txout is valid
            let store_txout = utxo.expect("valid utxo").output;
            if store_txout != *txout {
                return Err(ValidatePSBTError::TxOutMismatch);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod util_tests {
    use bitcoin::{psbt::Psbt, ScriptBuf, TxOut};
    use reth_btc_wallet::psbt::{PsbtExt, PsbtInputExt};

    use crate::{
        database,
        test::{create_psbt, create_tx, eth_vector_to_fixed_bytes, trusted_dealer_setup},
        util::*,
    };

    fn db_setup() -> database::Db {
        let tmpdir = tempfile::tempdir().unwrap();
        let dbdir = tmpdir.path().to_path_buf().join("db.db");

        database::Db::open(dbdir).unwrap()
    }

    #[test]
    fn should_perform_sanity_checks() {
        let db = db_setup();
        let psbt = create_psbt(2);
        let res = validate_psbt(&psbt, NO_FLAGS, 2, &db);
        assert!(res.is_ok());

        // No inputs
        let mut psbt = create_psbt(2);
        psbt.inputs.clear();
        let res = validate_psbt(&psbt, NO_FLAGS, 2, &db);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "inputs cannot be 0");

        // No outputs
        let mut psbt = create_psbt(2);
        psbt.outputs.clear();
        let res = validate_psbt(&psbt, NO_FLAGS, 2, &db);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "outputs cannot be 0");
    }

    #[test]
    fn should_look_for_utxo_in_db() {
        let db = db_setup();
        let psbt = create_psbt(1);
        let res = validate_psbt(&psbt, ROUND1, 2, &db);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "cannot find UTXO in db");

        let tx = psbt.clone().extract_tx().expect("valid tx");
        let utxo = database::Utxo {
            outpoint: tx.input[0].previous_output,
            output: psbt.inputs[0].witness_utxo.clone().unwrap(),
            eth_address: None,
        };

        db.store_utxo(&utxo).unwrap();
        db.flush().unwrap();
        let res = validate_psbt(&psbt, ROUND1, 2, &db);
        assert!(res.is_ok());
    }

    #[test]
    fn should_fail_if_eth_tweak_missing() {
        let db = db_setup();
        let mut psbt = create_psbt(1);
        let tx = psbt.clone().extract_tx().expect("valid tx");
        let eth = eth_vector_to_fixed_bytes(vec![0u8; 20]);
        let utxo = database::Utxo {
            outpoint: tx.input[0].previous_output,
            output: psbt.inputs[0].witness_utxo.clone().unwrap(),
            eth_address: Some(eth),
        };

        db.store_utxo(&utxo).unwrap();
        db.flush().unwrap();
        let res = validate_psbt(&psbt, ROUND1, 2, &db);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "eth tweak mismatch");

        psbt.inputs[0].set_eth_address(eth);
        let res = validate_psbt(&psbt, ROUND1, 2, &db);
        assert!(res.is_ok());
    }

    #[test]
    fn should_fail_if_tx_out_mismatch() {
        let db = db_setup();
        let mut psbt = create_psbt(1);
        let tx = psbt.clone().extract_tx().expect("valid tx");
        // use utxo value to avoid absurdly high fee rate error
        let utxo_value = psbt.inputs[0].witness_utxo.clone().unwrap().value;

        let utxo = database::Utxo {
            outpoint: tx.input[0].previous_output,
            output: psbt.inputs[0].witness_utxo.clone().unwrap(),
            eth_address: None,
        };

        psbt.inputs[0].witness_utxo = Some(TxOut {
            value: utxo_value,
            script_pubkey: ScriptBuf::from_hex("7e").expect("valid script"),
        });

        db.store_utxo(&utxo).unwrap();
        db.flush().unwrap();
        let res = validate_psbt(&psbt, ROUND1, 2, &db);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "txout mismatch");
    }

    #[test]
    fn round_1_transition_tests() {
        let db = db_setup();
        let mut psbt = create_psbt(1);
        let tx = psbt.clone().extract_tx().expect("valid tx");
        let utxo = database::Utxo {
            outpoint: tx.input[0].previous_output,
            output: psbt.inputs[0].witness_utxo.clone().unwrap(),
            eth_address: None,
        };

        db.store_utxo(&utxo).unwrap();
        db.flush().unwrap();
        let res = validate_psbt(&psbt, ROUND1_TRANSITION, 2, &db);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "invalid number of signing commitments");

        let frost_id1 = frost::Identifier::try_from(1u16).expect("valid id");
        let frost_id2 = frost::Identifier::try_from(2u16).expect("valid id");

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
        let (_, signing_commits1) = frost::round1::commit(key_package1.signing_share(), rng);
        let (_, signing_commits2) = frost::round1::commit(key_package2.signing_share(), rng);

        psbt.inputs[0].set_signing_commitment(frost_id1, &signing_commits1);
        psbt.inputs[0].set_signing_commitment(frost_id2, &signing_commits2);

        let res = validate_psbt(&psbt, ROUND1_TRANSITION, 2, &db);
        assert!(res.is_ok());

        // Round 2 at this point should pass as well. B/c we have not hit a limit in the number of
        // signatures
        let res = validate_psbt(&psbt, ROUND2, 2, &db);
        assert!(res.is_ok());
    }

    #[test]
    fn round2_psbt_validation_checks() {
        let db = db_setup();
        let mut psbt = create_psbt(1);
        let tx = psbt.clone().extract_tx().expect("valid tx");
        let utxo = database::Utxo {
            outpoint: tx.input[0].previous_output,
            output: psbt.inputs[0].witness_utxo.clone().unwrap(),
            eth_address: None,
        };
        let rng = &mut rand::thread_rng();

        db.store_utxo(&utxo).unwrap();
        db.flush().unwrap();

        let frost_id1 = frost::Identifier::try_from(1u16).expect("valid id");
        let (shares, _pk_package) = trusted_dealer_setup(2, 3);
        let key_package1 = frost::keys::KeyPackage::try_from(
            shares[&frost::Identifier::try_from(1u16).expect("valid id")].clone(),
        )
        .expect("valid key package");

        let key_package2 = frost::keys::KeyPackage::try_from(
            shares[&frost::Identifier::try_from(2u16).expect("valid id")].clone(),
        )
        .expect("valid key package");

        let (_, signing_commits1) = frost::round1::commit(key_package1.signing_share(), rng);
        psbt.inputs[0].set_signing_commitment(frost_id1, &signing_commits1);

        // Lets add two signatures and use min_signers = 1
        let sig_share1 =
            frost::round2::SignatureShare::deserialize([1u8; 32]).expect("valid sig share");
        let sig_share2 =
            frost::round2::SignatureShare::deserialize([2u8; 32]).expect("valid sig share");

        psbt.inputs[0].set_partial_signature(frost_id1, &sig_share1);

        // Should pass with 1 signature
        let res = validate_psbt(&psbt, ROUND2, 1, &db);
        assert!(res.is_ok());

        // Should fail with two signatures
        let frost_id2 = frost::Identifier::try_from(2u16).expect("valid id");
        psbt.inputs[0].set_partial_signature(frost_id2, &sig_share2);
        let res = validate_psbt(&psbt, ROUND2, 1, &db);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "invalid number of partial signatures");

        // Should fail ROUND2_TRANSITION since we havent added other signers signing commit
        let res = validate_psbt(&psbt, ROUND2_TRANSITION, 1, &db);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "invalid number of partial signatures");

        // Add other signing commit
        let (_, signing_commits2) = frost::round1::commit(key_package2.signing_share(), rng);
        psbt.inputs[0].set_signing_commitment(frost_id2, &signing_commits2);
        let res = validate_psbt(&psbt, ROUND2_TRANSITION, 2, &db);
        assert!(res.is_ok());

        // Should fail if there is another signer
        let frost_id3 = frost::Identifier::try_from(3u16).expect("valid id");
        let key_package3 = frost::keys::KeyPackage::try_from(
            shares[&frost::Identifier::try_from(3u16).expect("valid id")].clone(),
        )
        .expect("valid key package");

        let (_, signing_commits3) = frost::round1::commit(key_package3.signing_share(), rng);
        psbt.inputs[0].set_signing_commitment(frost_id3, &signing_commits3);
        let sig = frost::round2::SignatureShare::deserialize([3u8; 32]).expect("valid sig share");
        psbt.inputs[0].set_partial_signature(frost_id3, &sig);

        let res = validate_psbt(&psbt, ROUND2_TRANSITION, 2, &db);
        assert!(res.is_err());
    }

    #[test]
    fn convert_bdk_fee_rate() {
        let bdk_fee = bdk::bitcoin::FeeRate::from_sat_per_vb(10).unwrap();
        let rust_bitcoin_fee = bitcoin::FeeRate::from_sat_per_vb(10).unwrap();

        assert_eq!(bdk_fee, rust_bitcoin_fee);
    }

    #[test]
    fn should_add_signing_commits_to_psbt() {
        let num_inputs = 2;
        let frost_id1 = frost::Identifier::try_from(1u16).expect("valid id");
        let frost_id2 = frost::Identifier::try_from(2u16).expect("valid id");

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

        let tx = create_tx(num_inputs);

        let mut psbt = Psbt::from_unsigned_tx(tx.clone()).unwrap();
        // Add signing commitments to the psbt for each input
        psbt.inputs[0].set_signing_commitment(frost_id1, &signing_commits1_0);
        psbt.inputs[1].set_signing_commitment(frost_id1, &signing_commits1_1);
        psbt.inputs[0].set_signing_commitment(frost_id2, &signing_commits2_0);
        psbt.inputs[1].set_signing_commitment(frost_id2, &signing_commits2_1);

        // lets try to retrieve the signing commitments
        let retrieved_scs = (0..2)
            .map(|i| psbt.inputs[i].signing_commitments(frost_id1).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(retrieved_scs, vec![signing_commits1_0, signing_commits1_1]);

        let retrieved_scs = (0..2)
            .map(|i| psbt.inputs[i].signing_commitments(frost_id2).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(retrieved_scs, vec![signing_commits2_0, signing_commits2_1]);
    }

    #[test]
    fn should_add_partial_signatures_to_psbt() {
        let num_inputs = 2;
        let frost_id1 = frost::Identifier::try_from(1u16).expect("valid id");
        let frost_id2 = frost::Identifier::try_from(2u16).expect("valid id");

        let sig_share1_0 =
            frost::round2::SignatureShare::deserialize([1u8; 32]).expect("valid sig share");
        let sig_share1_1 =
            frost::round2::SignatureShare::deserialize([2u8; 32]).expect("valid sig share");

        let sig_share2_0 =
            frost::round2::SignatureShare::deserialize([3u8; 32]).expect("valid sig share");
        let sig_share2_1 =
            frost::round2::SignatureShare::deserialize([4u8; 32]).expect("valid sig share");

        let tx = create_tx(num_inputs);
        let mut psbt = Psbt::from_unsigned_tx(tx.clone()).unwrap();
        // Add signing commitments to the psbt for each input
        psbt.inputs[0].set_partial_signature(frost_id1, &sig_share1_0);
        psbt.inputs[1].set_partial_signature(frost_id1, &sig_share1_1);
        psbt.inputs[0].set_partial_signature(frost_id2, &sig_share2_0);
        psbt.inputs[1].set_partial_signature(frost_id2, &sig_share2_1);

        // lets try to retrieve
        let retrieved_sigs = (0..2)
            .map(|i| psbt.inputs[i].partial_signature(frost_id1).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(retrieved_sigs, vec![sig_share1_0, sig_share1_1]);

        let retrieved_sigs = (0..2)
            .map(|i| psbt.inputs[i].partial_signature(frost_id2).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(retrieved_sigs, vec![sig_share2_0, sig_share2_1]);
    }

    #[test]
    fn signing_package_conversion_should_fail_when_missing_signing_commitments() {
        let tx = create_tx(1);
        let mut psbt = Psbt::from_unsigned_tx(tx.clone()).unwrap();
        psbt.inputs[0].witness_utxo =
            Some(TxOut { value: Amount::from_sat(1000), script_pubkey: ScriptBuf::new() });

        let signing_packages = psbt.signing_packages();
        assert!(signing_packages.is_err());
        assert_eq!(
            signing_packages.unwrap_err().to_string(),
            reth_btc_wallet::psbt::PsbtToSigningPackageConversionError::MissingSigningCommitments
                .to_string()
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
        psbt.inputs[0].witness_utxo =
            Some(TxOut { value: Amount::from_sat(1000), script_pubkey: ScriptBuf::new() });
        psbt.inputs[1].witness_utxo =
            Some(TxOut { value: Amount::from_sat(1000), script_pubkey: ScriptBuf::new() });
        psbt.inputs[0].set_signing_commitment(frost_id1, &signing_commits1_0);
        psbt.inputs[1].set_signing_commitment(frost_id1, &signing_commits1_1);
        psbt.inputs[0].set_signing_commitment(frost_id2, &signing_commits2_0);
        psbt.inputs[1].set_signing_commitment(frost_id2, &signing_commits2_1);

        // Add a eth tweak to the first input
        let eth_tweak = eth_vector_to_fixed_bytes(vec![1u8; 20]);
        psbt.inputs[0].set_eth_address(eth_tweak);

        let signing_packages = psbt.signing_packages().expect("valid list signing package");
        assert_eq!(signing_packages.len(), 2);
        assert_eq!(
            signing_packages[0]
                .signing_commitments()
                .values()
                .copied()
                .collect::<Vec<frost::round1::SigningCommitments>>()
                .clone(),
            vec![signing_commits1_0, signing_commits2_0]
        );
        // check the frost ids as well
        assert_eq!(
            signing_packages[0]
                .signing_commitments()
                .keys()
                .copied()
                .collect::<Vec<frost::Identifier>>()
                .clone(),
            vec![frost_id1, frost_id2]
        );
        assert!(signing_packages[0].additional_tweak().as_ref().unwrap() == &eth_tweak.to_vec());

        assert_eq!(
            signing_packages[1]
                .signing_commitments()
                .values()
                .copied()
                .collect::<Vec<frost::round1::SigningCommitments>>()
                .clone(),
            vec![signing_commits1_1, signing_commits2_1]
        );
        // check the frost ids as well
        assert_eq!(
            signing_packages[1]
                .signing_commitments()
                .keys()
                .copied()
                .collect::<Vec<frost::Identifier>>()
                .clone(),
            vec![frost_id1, frost_id2]
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
        let f = frost::Identifier::derive(peer_id0.to_be_bytes().as_ref()).unwrap();
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
