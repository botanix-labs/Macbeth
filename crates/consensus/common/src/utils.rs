use crate::validation;
use reth_botanix_lib::extra_data_header::ExtraDataHeader;
use reth_interfaces::consensus::ConsensusError;
use reth_primitives::{
    constants::STAKING_CONTRACT_ADDRESS, keccak256, public_key_to_address, Address, Bytes, Header,
    B256, U256,
};
use reth_provider::StateProvider;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::error;

/// Error that can occur while accessing EVM global storage
#[derive(Debug)]
#[allow(dead_code)]
pub enum StorageAccessError {
    /// Failed to access storage
    FailedAccess(&'static str),
}

/// Create sighash for authority to sign
pub fn create_authority_sighash(header: &mut Header, extra_data: &ExtraDataHeader) -> B256 {
    // Remove the signature from the extra data header
    // And recalculate optional bitmask
    let mut extra_data_header_clone = extra_data.clone();
    extra_data_header_clone.authority_signature = None;
    extra_data_header_clone.set_optional_fields_bitmask();

    let mut writer: Vec<u8> = vec![];
    extra_data_header_clone
        .encode_into_without_signature(&mut writer)
        .expect("Valid extra data header");
    // Take ownership of the data in writer and leave an empty Vec<u8>
    let bytes_data = Bytes::from(writer.clone());
    header.extra_data = bytes_data;
    header.hash_slow()
}

/// Read staker balance from staking contract storage
/// TODO(armins) refactor needed, read comment below    
pub fn read_staker_balance(
    provider: impl StateProvider,
    _staker_address: Address,
) -> Result<U256, StorageAccessError> {
    let staking_contract_address = Address::from_slice(STAKING_CONTRACT_ADDRESS.as_bytes());
    let payload: Vec<Vec<u8>> = vec![];
    // And no longer supports `from_low_u64_le()`
    // payload
    // payload.push(staker_address.as_bytes().to_vec());
    //     .push(H160::from_low_u64_le(STAKER_BALANCE_MAPPING_STORAGE_SLOT_INDEX).as_bytes().
    // to_vec());

    let storage_key = keccak256(payload.into_iter().flatten().collect::<Vec<u8>>());
    let balance = provider
        .storage(staking_contract_address, storage_key)
        .map_err(|_e| StorageAccessError::FailedAccess("Failed to retrieve storage"))?
        // TODO remove unwrap
        .unwrap();

    Ok(balance)
}

#[derive(Debug)]
/// Error that can occur while recovering the authority list
pub enum RecoverAuthorityError {
    /// Signature is missing in the extra data
    NoSignaturePresentInExtraData,
    /// ecdsa Signature was not recoverable
    FailedToRecoverSigner(secp256k1::Error),
    /// Failed to deserialize the extra data
    FailedToDerserializeExtraData(
        reth_botanix_lib::extra_data_header::ExtraDataHeaderDeserialzeError,
    ),
    /// Failed to create the sighash that the authority signed
    FailedToCreateSigHash(secp256k1::Error),
}

/// Recover the authority that signed the block
pub fn recovery_authority(header: &Header) -> Result<secp256k1::PublicKey, RecoverAuthorityError> {
    let extra_data = reth_botanix_lib::extra_data_header::ExtraDataHeader::deserialize(
        &mut header.extra_data.to_vec().as_slice(),
    )
    .map_err(|e| RecoverAuthorityError::FailedToDerserializeExtraData(e))?;

    let sighash = create_authority_sighash(&mut header.clone(), &extra_data);
    let message = secp256k1::Message::from_slice(&sighash.as_slice())
        .map_err(|e| RecoverAuthorityError::FailedToCreateSigHash(e))?;

    if let Some(signature) = extra_data.authority_signature {
        let signer = signature
            .recover(&message)
            .map_err(|e| RecoverAuthorityError::FailedToRecoverSigner(e))?;
        return Ok(signer)
    }

    Err(RecoverAuthorityError::NoSignaturePresentInExtraData)
}

#[derive(Debug)]
/// Errors that can occur while reading the authority list from the block header
pub enum GetAuthoritiesError {
    /// Failed to deserialize the extra data
    FailedToRecoverAuthorityList(
        reth_botanix_lib::extra_data_header::ExtraDataHeaderDeserialzeError,
    ),
}

/// Recover the authority list from the block header
pub fn get_authority_list(
    header: &Header,
) -> Result<Option<Vec<secp256k1::PublicKey>>, GetAuthoritiesError> {
    let extra_data = reth_botanix_lib::extra_data_header::ExtraDataHeader::deserialize(
        &mut header.extra_data.to_vec().as_slice(),
    )
    .map_err(|e| GetAuthoritiesError::FailedToRecoverAuthorityList(e))?;

    Ok(extra_data.authority_signers)
}

/// Returns the unix timestamp in seconds
pub fn unix_timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Validate poa block beneficiary
pub fn validate_poa_block_beneficiary(
    header: &Header,
    authority_signers: &Vec<secp256k1::PublicKey>,
) -> Result<(), ConsensusError> {
    authority_signers
        .iter()
        .find(|pk| public_key_to_address(**pk) == header.beneficiary)
        .ok_or(ConsensusError::BlockBeneficiaryIsNotAuthority)?;
    Ok(())
}

/// Validate poa extra data header
pub fn validate_poa_extra_data_header(
    header: &Header,
    authority_signers: &Vec<secp256k1::PublicKey>,
) -> Result<(), ConsensusError> {
    // Skip over genesis
    if header.number == 0 {
        return Ok(())
    }
    // First run the basic validation
    validation::validate_header_extradata(header)?;

    // Attempt to deserialize the extra data header
    let extra_data = reth_botanix_lib::extra_data_header::ExtraDataHeader::deserialize(
        &mut header.extra_data.to_vec().as_slice(),
    )
    .map_err(|e| {
        error!("Failed to deserialize extra data header: {:?}", e);
        ConsensusError::ExtraDataInvalid
    })?;
    // Validate the authority signature and signature came from one of the authorities
    let sig_hash = create_authority_sighash(&mut header.clone(), &extra_data);
    extra_data.validate_authority_signature(&sig_hash.to_vec(), authority_signers).map_err(
        |e| {
            error!("Failed to validate authority signature: {:?}", e);
            ConsensusError::InvalidAuthoritySignature
        },
    )?;
    // TODO (armins) in the future this is where we would validate federation votes

    Ok(())
}

/// Validate against parent header errors
#[derive(Debug)]
pub enum ValidateAgainstParentError {
    /// Signer limit exceeded
    /// Could occur when signer is signings many blocks in the same turn
    SignerLimitExceeded,
    /// Failed to deserialize the extra data
    FailedToDerserializeExtraData(RecoverAuthorityError),
}

impl From<ValidateAgainstParentError> for ConsensusError {
    fn from(e: ValidateAgainstParentError) -> Self {
        match e {
            ValidateAgainstParentError::SignerLimitExceeded => ConsensusError::SignerLimitExceeded,
            ValidateAgainstParentError::FailedToDerserializeExtraData(_) => {
                ConsensusError::ExtraDataInvalid
            }
        }
    }
}

/// Validate current PoA header against parent header
pub fn validate_against_parent(
    parent: Header,
    current: Header,
) -> Result<(), ValidateAgainstParentError> {
    // Gensis block does not have a federation signature, skip
    if parent.number == 0 {
        return Ok(())
    }
    let parent_signer = recovery_authority(&parent).map_err(|e: RecoverAuthorityError| {
        ValidateAgainstParentError::FailedToDerserializeExtraData(e)
    })?;
    let current_signer = recovery_authority(&current)
        .map_err(|e| ValidateAgainstParentError::FailedToDerserializeExtraData(e))?;
    // Check if the parent block was mined in a different turn
    let parent_ts = parent.timestamp / 60;
    let current_ts = current.timestamp / 60;

    validate_current_signer_against_last((parent_signer, parent_ts), (current_signer, current_ts))?;

    Ok(())
}

/// Validate current signer and its last block timestamp against the last signer and its last block
/// timestamp Used to prevent a signer from signing multiple blocks in the same turn
pub fn validate_current_signer_against_last(
    last: (secp256k1::PublicKey, u64),
    current: (secp256k1::PublicKey, u64),
) -> Result<(), ValidateAgainstParentError> {
    // Last block should be greater that 1 minute in the worst cast
    // Even in the case of > 2 federation members the worst case time between blocks for the same
    // Signer should be 1 minute. Assuming 1 minute block times
    if last.0 == current.0 && current.1 - last.1 < 1 {
        return Err(ValidateAgainstParentError::SignerLimitExceeded)
    }

    Ok(())
}

mod tests {
    use std::str::FromStr;

    use super::*;

    #[allow(dead_code)]
    const EDH_DEFAULT_SIGHASH: &str =
        "0x0a088807360d347e57b95b64d765266f9551acc33ecfcdb2d49003a66acbf192";

    #[allow(dead_code)]
    const SK1: &str = "1aabc5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019";
    #[allow(dead_code)]
    const SK2: &str = "1bc1f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019";

    #[allow(dead_code)]
    fn generate_secret_key(hex_string: &str) -> secp256k1::SecretKey {
        secp256k1::SecretKey::from_str(hex_string).expect("Invalid hex string for SecretKey")
    }
    #[allow(dead_code)]
    fn sign_block_helper(header: &mut Header, signer_sk: Option<&str>) -> () {
        let mut edh = ExtraDataHeader::default();
        let sk1 = generate_secret_key(SK1);
        let sk2 = generate_secret_key(SK2);

        edh.authority_signers = Some(vec![
            secp256k1::PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), &sk1),
            secp256k1::PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), &sk2),
        ]);

        let secp = secp256k1::Secp256k1::new();
        header.number = 1;

        let sighash = create_authority_sighash(&mut header.clone(), &edh);
        let message = secp256k1::Message::from_slice(&sighash.as_slice()).unwrap();
        let signature = {
            if let Some(sk_str) = signer_sk {
                let sk = generate_secret_key(sk_str);
                secp256k1::Secp256k1::sign_ecdsa_recoverable(&secp, &message, &sk)
            } else {
                // By default sign with the first authority
                secp256k1::Secp256k1::sign_ecdsa_recoverable(&secp, &message, &sk1)
            }
        };
        edh.set_signature(signature);

        header.extra_data = Bytes::from(edh.serialize());
    }
    /* Tests for create authority sighash utility */
    #[test]
    fn create_default_edh_sighhash() {
        let edh = ExtraDataHeader::default();
        let mut header = Header::default();
        let sighash = create_authority_sighash(&mut header, &edh);

        assert_eq!(sighash.to_string(), EDH_DEFAULT_SIGHASH);
    }

    #[test]
    fn create_sighash_with_authority_signature() {
        // regarless of the signature, the sighash should be the same
        // This is because we remove the signature from the extra data header before signing
        let mut edh = ExtraDataHeader::default();
        edh.set_signature(
            secp256k1::ecdsa::RecoverableSignature::from_compact(
                &[0u8; 64],
                secp256k1::ecdsa::RecoveryId::from_i32(1i32).unwrap(),
            )
            .unwrap(),
        );
        let mut header = Header::default();
        let sighash = create_authority_sighash(&mut header, &edh);

        assert_eq!(sighash.to_string(), EDH_DEFAULT_SIGHASH);
    }
    #[test]
    fn create_sighash_with_authorities() {
        // However adding something else such as authority members should result in a different
        // sighash
        let mut edh = ExtraDataHeader::default();
        edh.authority_signers = Some(vec![
            secp256k1::PublicKey::from_secret_key(
                &secp256k1::Secp256k1::new(),
                &secp256k1::SecretKey::from_str(
                    "1aabc5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019",
                )
                .unwrap(),
            ),
            secp256k1::PublicKey::from_secret_key(
                &secp256k1::Secp256k1::new(),
                &secp256k1::SecretKey::from_str(
                    "1bc1f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019",
                )
                .unwrap(),
            ),
        ]);
        let mut header = Header::default();
        let sighash = create_authority_sighash(&mut header, &edh);

        assert_ne!(sighash.to_string(), EDH_DEFAULT_SIGHASH);
    }

    // Get authority list tests
    #[test]
    fn should_recover_none_authorities() {
        let edh = ExtraDataHeader::default();
        let mut header = Header::default();
        header.extra_data = Bytes::from(edh.serialize());
        let signer_list = get_authority_list(&header).unwrap();

        assert_eq!(signer_list, None);
    }

    #[test]
    fn should_recovery_authorities() {
        let mut header = Header::default();
        sign_block_helper(&mut header, None);
        let edh = ExtraDataHeader::deserialize(&mut header.extra_data.to_vec().as_slice()).unwrap();
        let signer_list = get_authority_list(&header).unwrap();

        assert_eq!(signer_list, edh.authority_signers);
    }

    #[test]
    fn fails_to_recover_when_edh_invalid() {
        let mut header = Header::default();
        header.extra_data = Bytes::from("foobar");
        let signer_list = get_authority_list(&header);

        assert!(signer_list.is_err());
    }

    // Tests for recover authority pk
    #[test]
    fn should_recover_authority() {
        let mut header = Header::default();
        sign_block_helper(&mut header, None);
        let edh = ExtraDataHeader::deserialize(&mut header.extra_data.to_vec().as_slice()).unwrap();

        let recovered = recovery_authority(&header).unwrap();
        // utility function above only signs with the first authority
        assert_eq!(recovered, edh.authority_signers.unwrap()[0]);
    }

    // Tests for validating poa extra data header
    #[test]
    fn should_skip_over_genesis() {
        let mut header = Header::default();
        header.number = 0;
        let authority_signers = vec![];
        let result = validate_poa_extra_data_header(&header, &authority_signers);

        assert!(result.is_ok());
    }

    #[test]
    fn should_fail_on_invalid_signature() {
        // In this case we are signing with a non federation different key
        let mut edh = ExtraDataHeader::default();
        let sk1 = secp256k1::SecretKey::from_str(
            "1aabc5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019",
        )
        .unwrap();
        let non_fed = secp256k1::SecretKey::from_str(
            "1bc1f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019",
        )
        .unwrap();

        edh.authority_signers =
            Some(vec![secp256k1::PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), &sk1)]);

        let secp = secp256k1::Secp256k1::new();
        let mut header = Header::default();
        header.number = 1;

        let sighash = create_authority_sighash(&mut header, &edh);
        let message = secp256k1::Message::from_slice(&sighash.as_slice()).unwrap();
        let signature = secp256k1::Secp256k1::sign_ecdsa_recoverable(&secp, &message, &non_fed);

        edh.set_signature(signature);

        header.extra_data = Bytes::from(edh.serialize());
        let authority_signers = vec![];
        let result = validate_poa_extra_data_header(&header, &authority_signers);
        assert!(result.is_err());
    }

    #[test]
    fn should_validate_poa_header() {
        // In this case we are signing with a non federation different key
        let mut header = Header::default();
        sign_block_helper(&mut header, None);
        let edh = ExtraDataHeader::deserialize(&mut header.extra_data.to_vec().as_slice()).unwrap();
        let authority_signers = edh.authority_signers.unwrap();
        let result = validate_poa_extra_data_header(&header, &authority_signers);
        assert!(result.is_ok());
    }

    #[test]
    fn unix_timestamp() {
        let timestamp = super::unix_timestamp();
        assert!(timestamp > 0);
    }

    #[test]
    fn should_fail_on_unlisted_poa_beneficiary() {
        let auth_signer1 = secp256k1::PublicKey::from_secret_key(
            &secp256k1::Secp256k1::new(),
            &secp256k1::SecretKey::from_str(
                "1aabc5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019",
            )
            .unwrap(),
        );
        let auth_signer2 = secp256k1::PublicKey::from_secret_key(
            &secp256k1::Secp256k1::new(),
            &secp256k1::SecretKey::from_str(
                "1bc1f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019",
            )
            .unwrap(),
        );
        let authority_signers = vec![auth_signer1, auth_signer2];

        // test bad signer
        let mut header = Header::default();
        let secret_key = secp256k1::SecretKey::from_str(
            "4646464646464646464646464646464646464646464646464646464646464646",
        )
        .unwrap();
        let beneficiary_pub_key =
            secp256k1::PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), &secret_key);
        let beneficiary_address = public_key_to_address(beneficiary_pub_key);
        header.beneficiary = beneficiary_address;
        assert!(validate_poa_block_beneficiary(&header, &authority_signers).is_err());

        // test good signer
        let mut header = Header::default();
        let beneficiary_address = public_key_to_address(auth_signer2);
        header.beneficiary = beneficiary_address;
        assert!(validate_poa_block_beneficiary(&header, &authority_signers).is_ok());
    }

    #[test]
    fn validate_against_parent_skip_gensis() {
        let mut parent = Header::default();
        parent.number = 0;
        let current = Header::default();
        let result = validate_against_parent(parent, current);
        assert!(result.is_ok());
    }

    #[test]
    fn should_fail_with_same_signer() {
        let mut parent = Header::default();
        let mut current = Header::default();

        parent.number = 1;
        current.number = 2;

        sign_block_helper(&mut parent, None);
        sign_block_helper(&mut current, None);

        let result = validate_against_parent(parent, current);
        assert!(result.is_err());
    }

    #[test]
    fn should_pass_after_sufficient_time() {
        let mut parent = Header::default();
        let mut current = Header::default();

        parent.number = 1;
        parent.timestamp = 1704834442 as u64;
        current.number = 2;
        current.timestamp = 1704834442 as u64 + 60;

        sign_block_helper(&mut parent, None);
        sign_block_helper(&mut current, None);

        let result = validate_against_parent(parent, current);
        assert!(result.is_ok());
    }

    #[test]
    fn should_pass_with_different_signer() {
        let mut parent = Header::default();
        let mut current = Header::default();
        parent.number = 1;
        current.number = 2;

        sign_block_helper(&mut parent, None);
        sign_block_helper(&mut current, Some(SK2));

        let result = validate_against_parent(parent, current);
        assert!(result.is_ok());
    }
}
