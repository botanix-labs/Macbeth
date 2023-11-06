use botanix_lib::extra_data_header::ExtraDataHeader;
use reth_primitives::{
    constants::{STAKER_BALANCE_MAPPING_STORAGE_SLOT_INDEX, STAKING_CONTRACT_ADDRESS},
    keccak256, Address, Bytes, Header, H160, H256, U256,
};
use reth_provider::StateProvider;

#[derive(Debug)]
pub(crate) enum StorageAccessError {
    FailedAccess(&'static str),
}

/// Create sighash for authority to sign
pub(crate) fn create_authority_sighash(header: &mut Header, extra_data: &ExtraDataHeader) -> H256 {
    header.extra_data = Bytes::from(extra_data.serialize_without_signature().as_slice());
    header.hash_slow()
}

/// Read staker balance from staking contract storage
pub(crate) fn read_staker_balance(
    provider: impl StateProvider,
    staker_address: Address,
) -> Result<U256, StorageAccessError> {
    let staking_contract_address = Address::from_slice(STAKING_CONTRACT_ADDRESS.as_bytes());
    let mut payload: Vec<Vec<u8>> = vec![];
    payload.push(staker_address.as_bytes().to_vec());
    payload
        .push(H160::from_low_u64_le(STAKER_BALANCE_MAPPING_STORAGE_SLOT_INDEX).as_bytes().to_vec());

    let storage_key = keccak256(payload.into_iter().flatten().collect::<Vec<u8>>());
    let balance = provider
        .storage(staking_contract_address, storage_key)
        .map_err(|_e| StorageAccessError::FailedAccess("Failed to retrieve storage"))?
        .unwrap();

    Ok(balance)
}

#[derive(Debug)]
pub(crate) enum RecoverAuthorityError {
    NoSignaturePresentInExtraData,
    FailedToRecoverSigner(secp256k1::Error),
    FailedToDerserializeExtraData(botanix_lib::extra_data_header::ExtraDataHeaderDeserialzeError),
    FailedToCreateSigHash(secp256k1::Error),
}

pub(crate) fn recovery_authority(
    header: &Header,
) -> Result<secp256k1::PublicKey, RecoverAuthorityError> {
    let extra_data =
        botanix_lib::extra_data_header::ExtraDataHeader::deserialize(header.extra_data.to_vec())
            .map_err(|e| RecoverAuthorityError::FailedToDerserializeExtraData(e))?;

    let sighash = create_authority_sighash(&mut header.clone(), &extra_data);
    let message = secp256k1::Message::from_slice(sighash.as_bytes())
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
pub(crate) enum GetAuthoritiesError {
    FailedToRecoverAuthorityList(botanix_lib::extra_data_header::ExtraDataHeaderDeserialzeError),
}

pub(crate) fn get_authority_list(
    header: &Header,
) -> Result<Vec<secp256k1::PublicKey>, GetAuthoritiesError> {
    let extra_data =
        botanix_lib::extra_data_header::ExtraDataHeader::deserialize(header.extra_data.to_vec())
            .map_err(|e| GetAuthoritiesError::FailedToRecoverAuthorityList(e))?;

    Ok(extra_data.authority_signers)
}
