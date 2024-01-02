use reth_botanix_lib::extra_data_header::ExtraDataHeader;
use reth_primitives::{
    constants::{STAKER_BALANCE_MAPPING_STORAGE_SLOT_INDEX, STAKING_CONTRACT_ADDRESS},
    keccak256, Address, Bytes, Header, H160, H256, U256,
};
use reth_provider::StateProvider;

#[derive(Debug)]
pub enum StorageAccessError {
    FailedAccess(&'static str),
}

#[derive(Debug, thiserror::Error)]
pub enum StateProviderError {
    #[error("Storage Access Error")]
    StorageAccessError(&'static str),
}

/// Create sighash for authority to sign
pub fn create_authority_sighash(header: &mut Header, extra_data: &ExtraDataHeader) -> H256 {
    let mut writer: Vec<u8> = vec![];
    extra_data.encode_into_without_signature(&mut writer).expect("Valid extra data header");

    // Take ownership of the data in writer and leave an empty Vec<u8>
    let bytes_data = Bytes::from(writer.clone());

    header.extra_data = bytes_data;

    header.hash_slow()
}

/// Read staker balance from staking contract storage
/// TODO(armins) refactor needed, read comment below    
pub fn read_staker_balance(
    provider: impl StateProvider,
    staker_address: Address,
) -> Result<U256, StorageAccessError> {
    let staking_contract_address = Address::from_slice(STAKING_CONTRACT_ADDRESS.as_bytes());
    let mut payload: Vec<Vec<u8>> = vec![];
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
pub enum RecoverAuthorityError {
    NoSignaturePresentInExtraData,
    FailedToRecoverSigner(secp256k1::Error),
    FailedToDerserializeExtraData(
        reth_botanix_lib::extra_data_header::ExtraDataHeaderDeserialzeError,
    ),
    FailedToCreateSigHash(secp256k1::Error),
}

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
pub enum GetAuthoritiesError {
    FailedToRecoverAuthorityList(
        reth_botanix_lib::extra_data_header::ExtraDataHeaderDeserialzeError,
    ),
}

pub fn get_authority_list(
    header: &Header,
) -> Result<Vec<secp256k1::PublicKey>, GetAuthoritiesError> {
    let extra_data = reth_botanix_lib::extra_data_header::ExtraDataHeader::deserialize(
        &mut header.extra_data.to_vec().as_slice(),
    )
    .map_err(|e| GetAuthoritiesError::FailedToRecoverAuthorityList(e))?;

    Ok(extra_data.authority_signers)
}
