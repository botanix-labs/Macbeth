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

#[derive(Debug, Error)]
pub enum StateProviderError {
    #[error("Storage Access Error")]
    StorageAccessError(&'static str),
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
    payload.push(H160::from_low_u64_le(STAKER_BALANCE_MAPPING_STORAGE_SLOT_INDEX).as_bytes().to_vec());
    

    let storage_key = keccak256(payload.into_iter().flatten().collect::<Vec<u8>>());
    let balance = provider
        .storage(staking_contract_address, storage_key)
        .map_err(|_e| StorageAccessError::FailedAccess("Failed to retrieve storage"))?.unwrap();

    Ok(balance)
}

/// Read staker balance from staking contract storage
pub fn read_staker_balance(provider: StateProvider, staker_address: Address) -> Result<U256, StorageAccessError> {
    let staking_contract_address = Address::from_slice(&STAKING_CONTRACT_ADDRESS);
    let staker_balance_mapping_storage_slot_index = STAKER_BALANCE_MAPPING_STORAGE_SLOT_INDEX;

    let storage_key = keccak256(&[staker_address, staker_balance_mapping_storage_slot_index]);
    let balance = provider.storage(staking_contract_address, storage_key).map_err(|_e| {
        StateProviderError::StorageAccessError("Failed to access staking contract mapping storage slot");
    })?;
    

    let storage_key = keccak256(payload.into_iter().flatten().collect::<Vec<u8>>());
    let balance = provider
        .storage(staking_contract_address, storage_key)
        .map_err(|_e| StorageAccessError::FailedAccess("Failed to retrieve storage"))?.unwrap();

    Ok(balance)
}
