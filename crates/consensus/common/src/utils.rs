
use crate::validation;
use reth_botanix_lib::extra_data_header::ExtraDataHeader;
use reth_interfaces::consensus::ConsensusError;
use reth_primitives::{
    constants::STAKING_CONTRACT_ADDRESS, keccak256, Address, Bytes, Header, B256, U256,
};
use reth_provider::StateProvider;
use tracing::error;
use std::time::{SystemTime, UNIX_EPOCH};

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
    // TODO (armins) commenting out for now, need to refactor to not use H160 as it is deprecated
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
    let message = secp256k1::Message::from_slice(&sighash.0)
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
mod tests {
    use std::str::FromStr;

    use secp256k1::ecdsa::RecoveryId;
    use super::*;

    const EDH_DEFAULT_SIGHASH: &str = "0x0a088807360d347e57b95b64d765266f9551acc33ecfcdb2d49003a66acbf192";
    /* Tests for create authority sighash utility */
    #[test]
    fn create_default_edh_sighhash() {
        let edh = ExtraDataHeader::default();
        let mut header = Header::default();
        let sighash = create_authority_sighash(&mut header, &edh);

        assert_eq!(
            sighash.to_string(),
            EDH_DEFAULT_SIGHASH 
        );
    }

    #[test]
    fn create_sighash_with_authority_signature() {
        // regarless of the signature, the sighash should be the same
        // This is because we remove the signature from the extra data header before signing
        let mut edh = ExtraDataHeader::default();
        edh.authority_signature = Some(
            secp256k1::ecdsa::RecoverableSignature::from_compact(
                &[0u8; 64],
                RecoveryId::from_i32(1i32).unwrap(),
            )
            .unwrap(),
        );
        let mut header = Header::default();
        let sighash = create_authority_sighash(&mut header, &edh);

        assert_eq!(
            sighash.to_string(),
            EDH_DEFAULT_SIGHASH
        );
    }
    #[test]
    fn create_sighash_with_authorities() {
        // However adding something else such as authority members should result in a different sighash
        let mut edh = ExtraDataHeader::default();
        edh.authority_signers = Some(vec![
            secp256k1::PublicKey::from_secret_key(
                &secp256k1::Secp256k1::new(),
                &secp256k1::SecretKey::from_str("1aabc5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019").unwrap(),
            ),
            secp256k1::PublicKey::from_secret_key(
                &secp256k1::Secp256k1::new(),
                &secp256k1::SecretKey::from_str("1bc1f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019").unwrap()
            ),
        ]);
        let mut header = Header::default();
        let sighash = create_authority_sighash(&mut header, &edh);

        assert_ne!(
            sighash.to_string(),
            EDH_DEFAULT_SIGHASH
        );
    }

    #[test]
    fn unix_timestamp() {
        let timestamp = super::unix_timestamp();
        assert!(timestamp > 0);
    }
}
