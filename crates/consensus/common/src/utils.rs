use crate::validation;
use reth_consensus::ConsensusError;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_primitives::{
    constants::STAKING_CONTRACT_ADDRESS,
    header_ext::{GetAuthoritiesError, HeaderExt, RecoverAuthorityError},
    keccak256, public_key_to_address, Address, ChainSpec, Header, U256,
};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProvider, StateProviderFactory};
use reth_tracing::tracing::error;
use secp256k1::{All, Secp256k1};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

/// Error that can occur while accessing EVM global storage
#[derive(Debug)]
#[allow(dead_code)]
pub enum StorageAccessError {
    /// Failed to access storage
    FailedAccess(&'static str),
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

/// Returns
/// - The index of the authority that is currently in turn
/// - The list of all authorities
/// - The public key of the authority
pub fn get_authority_signer_index<Client>(
    client: Client,
    chain_spec: Arc<ChainSpec>,
    secp: Secp256k1<All>,
    sk: secp256k1::SecretKey,
) -> Result<(usize, Vec<secp256k1::PublicKey>, secp256k1::PublicKey), GetAuthoritiesError>
where
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
{
    let mut latest_header =
        client.latest_header().ok().flatten().unwrap_or_else(|| chain_spec.sealed_genesis_header());
    let mut headers = vec![latest_header.clone()];

    while !latest_header.header().is_poa_epoch() {
        let parent_hash = latest_header.parent_hash;

        if let Some(new_header) = client.header(&parent_hash).ok().flatten() {
            let old_latest_header = std::mem::replace(&mut latest_header, new_header.seal_slow());
            headers.push(old_latest_header);
        } else {
            return Err(GetAuthoritiesError::FailedToRetrieveEpochHeader);
        }
    }

    // Latest epoch header is the last header in the vector
    // This header should include the authority list which is validated by consensus
    let authorities =
        latest_header.get_authority_list()?.expect("authority signer list in epoch block");

    let authority_pk = sk.public_key(&secp);
    let signer_index = authorities.iter().position(|a| *a == sk.public_key(&secp));

    Ok((
        signer_index.ok_or(GetAuthoritiesError::FailedToFindAuthoritySignerIndex)?,
        authorities,
        authority_pk,
    ))
}

/// Returns the unix timestamp in seconds
pub fn unix_timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

// TODO move this into header ext
// not in authority utils because of circular dependency
/// Get the authority address from the header
pub fn get_block_producer_address(header: &Header) -> Address {
    if let Ok(authorities) = header.recovered_signed_authorities() {
        // TODO remove this unwrap
        let block_builder_public_key =
            authorities.get(0).expect("block producer authority to be present");
        return public_key_to_address(*block_builder_public_key)
    }

    // TODO this method should return a Result
    Address::ZERO
}
// not in authority utils because of circular dependency
/// Calculate the block reward split between botanix and the beneficiary
pub fn block_fees_split(total_block_fees: u128) -> (u128, u128) {
    // 20% of the block reward
    let botanix_reward = total_block_fees / 5;
    let beneficiary_reward = total_block_fees - botanix_reward;
    (botanix_reward, beneficiary_reward)
}

/// Validate poa extra data header
/// This function will validate the extra data header and check for a quorum of signatures
/// from authorities memebers.
/// TODO (armins) validate only 2/3 of the authorities have signed, rn we are checking for n
pub fn validate_poa_extra_data_header_single_signer(
    header: &Header,
    authority_signers: &[secp256k1::PublicKey],
) -> Result<(), ConsensusError> {
    // Skip over genesis
    if header.number == 0 {
        return Ok(());
    }
    // First run the basic validation
    validation::validate_header_extradata(header)?;

    // Attempt to deserialize the extra data header
    let _edh = header.deserialize_extra_data_header().map_err(|e| {
        error!("Failed to deserialize extra data header: {:?}", e);
        ConsensusError::ExtraDataInvalid
    })?;
    // Validate the authority signature and signature came from one of the authorities
    header.validate_first_authority_signature(authority_signers).map_err(|e| {
        error!("Failed to validate authority signature: {:?}", e);
        ConsensusError::InvalidAuthoritySignature
    })?;

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
        return Ok(());
    }
    let parent_signer = parent
        .recovered_signed_authorities()
        .map_err(|e| ValidateAgainstParentError::FailedToDerserializeExtraData(e))?[0];
    let current_signer = current
        .recovered_signed_authorities()
        .map_err(ValidateAgainstParentError::FailedToDerserializeExtraData)?[0];
    // Check if the parent block was mined in a different turn
    let parent_ts = parent.timestamp as f64 / 60.0;
    let current_ts = current.timestamp as f64 / 60.0;

    validate_current_signer_against_last((parent_signer, parent_ts), (current_signer, current_ts))?;

    Ok(())
}

/// Validate current signer and its last block timestamp against the last signer and its last block
/// timestamp Used to prevent a signer from signing multiple blocks in the same turn
pub fn validate_current_signer_against_last(
    last: (secp256k1::PublicKey, f64),
    current: (secp256k1::PublicKey, f64),
) -> Result<(), ValidateAgainstParentError> {
    // Last block should be greater that 1 minute in the worst cast
    // Even in the case of > 2 federation members the worst case time between blocks for the same
    // Signer should be 1 minute. Assuming 1 minute block times
    if last.0 == current.0 && current.1 - last.1 < 1.0 {
        return Err(ValidateAgainstParentError::SignerLimitExceeded);
    }

    Ok(())
}

/// Returns true if the authority is in turn
pub fn is_inturn(authorities_len: u64, signer_index: u64) -> bool {
    let timestamp = unix_timestamp(); // Keep the timestamp in seconds
    let cycle_length = authorities_len * 60; // Full cycle length in seconds

    // Calculate the position in the current cycle
    let position_in_cycle = timestamp % cycle_length;

    // Determine the current signer index based on the position in the cycle
    // Each signer's turn lasts for 60 seconds
    (position_in_cycle / 60) % authorities_len == signer_index
}

/// Typedef for (start of current turn, end of current turn, time taken, time remaining)
pub type CoordinatorInterval = (u64, u64, u64, u64);

/// Returns the inturn interval for a signer index based on the seconds passed
pub fn get_in_turn_interval(
    authorities_len: u64,
    signer_index: u64,
    reference_timestamp: u64,
) -> CoordinatorInterval {
    // Calculate the length of one complete cycle
    let cycle_length = authorities_len * 60;

    // Calculate how many complete cycles have passed since the epoch
    let cycles_since_epoch = reference_timestamp / cycle_length;

    // Calculate the start time of the current cycle
    let current_cycle_start = cycles_since_epoch * cycle_length;

    // Calculate the start time of the current turn for the given signer_index
    let start_of_current_turn = current_cycle_start + (signer_index * 60);

    // End time of the current turn, ensuring full 60 seconds
    let end_of_current_turn = start_of_current_turn + 59;

    (
        start_of_current_turn,
        end_of_current_turn,
        reference_timestamp - start_of_current_turn,
        end_of_current_turn - reference_timestamp,
    )
}

/// Returns the index of the authority which is currently in turn based on the seconds passed
pub fn current_inturn_index(authorities_len: u64, reference_timestamp: u64) -> u64 {
    // Calculate the length of one complete cycle
    let cycle_length = authorities_len * 60;

    // Calculate the position in the current cycle
    let position_in_cycle = reference_timestamp % cycle_length;

    // Determine the current signer index based on the position in the cycle
    (position_in_cycle / 60) % authorities_len
}
