use botanix_lib::extra_data_header::ExtraDataHeader;
use reth_primitives::{constants::{ STAKING_CONTRACT_ADDRESS, STAKER_BALANCE_MAPPING_STORAGE_SLOT_INDEX }, Address, Header, H256, keccak256, U256};
use reth_provider::StateProvider;

pub enum CreateSigHashError {
    
}


#[derive(Debug, Error)]
pub enum StorageAccessError {
    #[error("Failed to access staking contract mapping storage slot")]
    FailedAccess(&'static str),
}

/// Create sighash for authority to sign
pub fn create_authority_sighash(header: &mut Header, extra_data: &ExtraDataHeader) -> H256 {
    header.extra_data = Bytes::from(extra_data.serialize_without_signature().as_slice());

    header.hash_slow()
}

/// Read staker balance from staking contract storage
pub fn read_staker_balance(provider: StateProvider, staker_address: Address) -> Result<U256, StorageAccessError> {
    let staking_contract_address = Address::from_slice(&STAKING_CONTRACT_ADDRESS);
    let staker_balance_mapping_storage_slot_index = STAKER_BALANCE_MAPPING_STORAGE_SLOT_INDEX;

    let storage_key = keccak256(&[staker_address, staker_balance_mapping_storage_slot_index]);
    let balance = provider.storage(staking_contract_address, storage_key).map_err(|_e| {
        StorageAccessError::FailedAccess(())
    })?;
    
    Ok(balance)
}