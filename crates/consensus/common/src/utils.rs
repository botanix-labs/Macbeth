use reth_botanix_lib::extra_data_header::ExtraDataHeader;
use reth_consensus::ConsensusError;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_primitives::{
    constants::STAKING_CONTRACT_ADDRESS, keccak256, public_key_to_address, Address, Bytes,
    ChainSpec, Header, B256, U256,
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
    .map_err(RecoverAuthorityError::FailedToDerserializeExtraData)?;

    let sighash = create_authority_sighash(&mut header.clone(), &extra_data);
    let message = secp256k1::Message::from_digest_slice(sighash.as_slice())
        .map_err(RecoverAuthorityError::FailedToCreateSigHash)?;

    if let Some(signature) = extra_data.authority_signature {
        let signer =
            signature.recover(&message).map_err(RecoverAuthorityError::FailedToRecoverSigner)?;
        return Ok(signer);
    }

    Err(RecoverAuthorityError::NoSignaturePresentInExtraData)
}

impl From<RecoverAuthorityError> for ConsensusError {
    fn from(e: RecoverAuthorityError) -> Self {
        match e {
            RecoverAuthorityError::FailedToRecoverSigner(_) => {
                ConsensusError::TransactionSignerRecoveryError
            }
            RecoverAuthorityError::FailedToCreateSigHash(_) |
            RecoverAuthorityError::FailedToDerserializeExtraData(_) |
            RecoverAuthorityError::NoSignaturePresentInExtraData => {
                ConsensusError::ExtraDataInvalid
            }
        }
    }
}
#[derive(Debug)]
/// Errors that can occur while reading the authority list from the block header
pub enum GetAuthoritiesError {
    /// Failed to deserialize the extra data
    FailedToRecoverAuthorityList(
        reth_botanix_lib::extra_data_header::ExtraDataHeaderDeserialzeError,
    ),
    /// Failed to retrive epoch header
    FailedToRetrieveEpochHeader,
    /// Failed to find authority index
    FailedToFindAuthoritySignerIndex,
}

/// Recover the authority list from the block header
pub fn get_authority_list(
    header: &Header,
) -> Result<Option<Vec<secp256k1::PublicKey>>, GetAuthoritiesError> {
    let extra_data = reth_botanix_lib::extra_data_header::ExtraDataHeader::deserialize(
        &mut header.extra_data.to_vec().as_slice(),
    )
    .map_err(GetAuthoritiesError::FailedToRecoverAuthorityList)?;

    Ok(extra_data.authority_signers)
}

/// Returns the authority signer index
pub fn get_authority_signer_index<Client>(
    client: Client,
    chain_spec: Arc<ChainSpec>,
    secp: Secp256k1<All>,
    sk: secp256k1::SecretKey,
) -> Result<(usize, Vec<secp256k1::PublicKey>), GetAuthoritiesError>
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
        get_authority_list(&latest_header)?.expect("authority signer list in epoch block");

    let signer_index = authorities.iter().position(|a| *a == sk.public_key(&secp));

    Ok((signer_index.ok_or(GetAuthoritiesError::FailedToFindAuthoritySignerIndex)?, authorities))
}

/// Returns the unix timestamp in seconds
pub fn unix_timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

// /// Validate poa block beneficiary
// pub fn validate_poa_block_beneficiary(header: &Header) -> Result<(), ConsensusError> {
//     if header.beneficiary != Address::ZERO {
//         return Err(ConsensusError::BlockBeneficiaryIsNotBurnAddress);
//     }

//     Ok(())
// }

// /// Validate poa extra data header
// pub fn validate_poa_extra_data_header(
//     header: &Header,
//     authority_signers: &[secp256k1::PublicKey],
// ) -> Result<(), ConsensusError> {
//     // Skip over genesis
//     if header.number == 0 {
//         return Ok(());
//     }
//     // First run the basic validation
//     validation::validate_header_extradata(header)?;

//     // Attempt to deserialize the extra data header
//     let extra_data = reth_botanix_lib::extra_data_header::ExtraDataHeader::deserialize(
//         &mut header.extra_data.to_vec().as_slice(),
//     )
//     .map_err(|e| {
//         error!("Failed to deserialize extra data header: {:?}", e);
//         ConsensusError::ExtraDataInvalid
//     })?;
//     // Validate the authority signature and signature came from one of the authorities
//     let sig_hash = create_authority_sighash(&mut header.clone(), &extra_data);
//     extra_data.validate_authority_signature(&sig_hash.to_vec(), authority_signers).map_err(
//         |e| {
//             error!("Failed to validate authority signature: {:?}", e);
//             ConsensusError::InvalidAuthoritySignature
//         },
//     )?;
//     // TODO (armins) in the future this is where we would validate federation votes

//     Ok(())
// }

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
    let parent_signer = recovery_authority(&parent).map_err(|e: RecoverAuthorityError| {
        ValidateAgainstParentError::FailedToDerserializeExtraData(e)
    })?;
    let current_signer = recovery_authority(&current)
        .map_err(ValidateAgainstParentError::FailedToDerserializeExtraData)?;
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
    // use minutes as time unit to determine in turn
    let timestamp = unix_timestamp() / 60;

    (timestamp / authorities_len) % authorities_len == signer_index
}

/// Typedef for (start of current turn, end of current turn, time taken, time remaining)
pub type CoordinatorInterval = (u64, u64, u64, u64);

/// Returns the inturn interval for a signer index
pub fn get_in_turn_interval(authorities_len: u64, signer_index: u64) -> CoordinatorInterval {
    let timestamp = unix_timestamp();
    let current_minute = timestamp / 60;
    let current_interval = current_minute / authorities_len;

    let start_of_current_turn = (current_interval * authorities_len + signer_index) * 60;
    let end_of_current_turn = (start_of_current_turn + authorities_len * 60) - 1;

    (
        start_of_current_turn,
        end_of_current_turn,
        timestamp - start_of_current_turn,
        end_of_current_turn - timestamp,
    )
}

/// Returns the index of the authority which is currently in turn
pub fn current_inturn_index(authorities_len: u64) -> u64 {
    // use minutes as time unit to determine in turn
    let timestamp = unix_timestamp() / 60;
    (timestamp / authorities_len) % authorities_len
}

/// Validates that the authority was in turn when producing the block
pub fn validate_inturn(
    header: &Header,
    authority_signers: &[secp256k1::PublicKey],
) -> Result<(), ConsensusError> {
    let singer_pk = recovery_authority(header)?;
    let signer_index = authority_signers
        .iter()
        .position(|pk| *pk == singer_pk)
        .ok_or(ConsensusError::AuthorityNotInTurn)?;

    let authorities_len = authority_signers.len() as u64;
    let block_timestamp_min = header.timestamp / 60;
    if (block_timestamp_min / authorities_len) % authorities_len != (signer_index as u64) {
        error!(target = "authority_consensus", "Authority was not in turn when producing block");
        return Err(ConsensusError::AuthorityNotInTurn);
    }

    Ok(())
}

// not in authority utils because of circular dependency
/// Get the authority address from the header
/// Return zero address if the authority is not present which is the case for reth tests
pub fn get_block_producer_address(header: &Header) -> Address {
    match recovery_authority(header) {
        Ok(pk) => public_key_to_address(pk),
        Err(_) => Address::ZERO,
    }
}
// not in authority utils because of circular dependency
/// Calculate the block reward split between botanix and the beneficiary
pub fn block_fees_split(total_block_fees: u128) -> (u128, u128) {
    // 20% of the block reward
    let botanix_reward = total_block_fees / 5;
    let beneficiary_reward = total_block_fees - botanix_reward;
    (botanix_reward, beneficiary_reward)
}

