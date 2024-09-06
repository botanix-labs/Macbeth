use reth_consensus::ConsensusError;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_primitives::{
    extra_data_header::CHAIN_VERSION,
    header_ext::{GetAuthoritiesError, HeaderExt, RecoverAuthorityError},
    public_key_to_address, Address, ChainSpec, Header,
};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};

use secp256k1::{All, Secp256k1};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::error;

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
    if let Ok(block_producer_address) = header.block_producer_address() {
        return block_producer_address;
    }

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

/// Validate poa block beneficiary
pub fn validate_poa_block_beneficiary(header: &Header) -> Result<(), ConsensusError> {
    if header.beneficiary != Address::ZERO {
        return Err(ConsensusError::BlockBeneficiaryIsNotBurnAddress);
    }

    Ok(())
}

/// Check authorities in EDH match and are in the same order and the genesis authorities
pub fn validate_extra_data_header_authorities(
    header: &Header,
    genesis_authorities: &[secp256k1::PublicKey],
) -> Result<(), ConsensusError> {
    if header.is_poa_epoch() {
        // Attempt to deserialize the extra data header
        let edh = header.deserialize_extra_data_header().map_err(|e| {
            error!("Failed to deserialize extra data header: {:?}", e);
            ConsensusError::ExtraDataInvalid
        })?;

        // Validate the list of authorities matches the authorities in the genesis block
        // This check is only for a static federation
        if let Some(authority_signers) = edh.authority_signers.as_ref() {
            if genesis_authorities != authority_signers {
                error!("Genesis authorities: {:?}", genesis_authorities);
                error!("EDH authorities: {:?}", edh.authority_signers);
                return Err(ConsensusError::InvalidAuthorityList);
            }
        } else {
            // error!("No authority signers in extra data header");
            return Err(ConsensusError::MissingAuthorityList);
        }
    }

    Ok(())
}

/// Check the extra data header field has the current chain version
pub fn validate_chain_version(edh_chain_version: u32) -> Result<(), ConsensusError> {
    if edh_chain_version != CHAIN_VERSION {
        return Err(ConsensusError::InvalidChainVersion);
    }

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
    block_time: u64,
) -> Result<(), ValidateAgainstParentError> {
    // Gensis block does not have a federation signature, skip
    if parent.number == 0 {
        return Ok(());
    }
    let parent_signer = parent
        .recovered_signed_authorities()
        .map_err(ValidateAgainstParentError::FailedToDerserializeExtraData)?[0];
    let current_signer = current
        .recovered_signed_authorities()
        .map_err(ValidateAgainstParentError::FailedToDerserializeExtraData)?[0];
    Ok(())
}

/// Validate current signer and its last block timestamp against the last signer and its last block
/// timestamp Used to prevent a signer from signing multiple blocks in the same turn
/// Assuming sane timestamps
pub fn validate_current_signer_against_last(
    last: (secp256k1::PublicKey, f64),
    current: (secp256k1::PublicKey, f64),
    block_time: u64,
) -> Result<(), ValidateAgainstParentError> {
    // Last block should be greater that `block_time` in the worst case
    // Even in the case of > 2 federation members the worst case time between blocks for the same
    // signer is 2 * block_time
    if last.0 == current.0 && (current.1 - last.1) < (block_time * 2) as f64 {
        return Err(ValidateAgainstParentError::SignerLimitExceeded);
    }

    Ok(())
}

/// Returns true if the authority is in turn
pub fn is_inturn(authorities_len: u64, signer_index: u64, block_time: u64) -> bool {
    let timestamp = unix_timestamp(); // Keep the timestamp in seconds
    let cycle_length = authorities_len * block_time; // Full cycle length in seconds

    // Calculate the position in the current cycle
    let position_in_cycle = timestamp % cycle_length;

    // Determine the current signer index based on the position in the cycle
    // Each signer's turn lasts for `block_time` seconds
    (position_in_cycle / block_time) % authorities_len == signer_index
}

/// Typedef for (start of current turn, end of current turn, time taken, time remaining)
pub type CoordinatorInterval = (u64, u64, u64, u64);

/// Returns the inturn interval for a signer index based on the seconds passed
pub fn get_in_turn_interval(
    authorities_len: u64,
    signer_index: u64,
    reference_timestamp: u64,
    block_time: u64,
) -> CoordinatorInterval {
    assert!(block_time > 0, "block_time must be greater than 0");
    // Calculate the length of one complete cycle
    let cycle_length = authorities_len * block_time;

    // Calculate how many complete cycles have passed since the epoch
    let cycles_since_epoch = reference_timestamp / cycle_length;

    // Calculate the start time of the current cycle
    let current_cycle_start = cycles_since_epoch * cycle_length;

    // Calculate the start time of the current turn for the given signer_index
    let start_of_current_turn = current_cycle_start + (signer_index * block_time);

    // End time of the current turn, ensuring full `block_time` seconds
    let end_of_current_turn = start_of_current_turn + block_time - 1;

    (
        start_of_current_turn,
        end_of_current_turn,
        reference_timestamp - start_of_current_turn,
        end_of_current_turn - reference_timestamp,
    )
}

/// Returns the index of the authority which is currently in turn based on the seconds passed
pub fn current_inturn_index(
    authorities_len: u64,
    reference_timestamp: u64,
    block_time: u64,
) -> u64 {
    // Calculate the length of one complete cycle
    let cycle_length = authorities_len * block_time;

    // Calculate the position in the current cycle
    let position_in_cycle = reference_timestamp % cycle_length;

    // Determine the current signer index based on the position in the cycle
    (position_in_cycle / block_time) % authorities_len
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use reth_primitives::{extra_data_header::ExtraDataHeader, header_ext::HeaderExt, Bytes};

    use super::*;

    #[test]
    fn unix_timestamp() {
        let timestamp = super::unix_timestamp();
        assert!(timestamp > 0);
    }

    #[test]
    fn should_validate_poa_block_beneficiary() {
        // default beneficiary is the burn address
        let header = Header::default();
        let result = validate_poa_block_beneficiary(&header);
        assert!(result.is_ok());
    }

    #[test]
    fn should_fail_validate_poa_block_beneficiary() {
        let mut header = Header::default();
        header.beneficiary =
            Address::from_str("0x4e0f6e05C8ca4b3dc2B7b7Ad6249B149b1980394").unwrap();
        let result = validate_poa_block_beneficiary(&header);
        assert!(result.is_err());
    }

    #[test]
    fn should_split_rewards() {
        let base_block_reward = 100;
        let (botanix_reward, beneficiary_reward) = block_fees_split(base_block_reward);
        assert_eq!(botanix_reward, 20);
        assert_eq!(beneficiary_reward, 80);
    }

    #[test]
    fn get_inturn_interval_secs_based() {
        let current_ts = super::unix_timestamp();
        let authorities_len = 10;
        let current_in_turn_signer =
            current_inturn_index(authorities_len, current_ts, BLOCK_TIME_SECONDS);
        let (start, end, time_passed, time_remaining) = get_in_turn_interval(
            authorities_len,
            current_in_turn_signer,
            current_ts,
            BLOCK_TIME_SECONDS,
        );

        println!(
            "Signer index {} is in turn from {}s to {}s. Current ts = {:?}s. Time passed = {:?}s, time remaining = {:?}s",
            current_in_turn_signer,
            start,
            end,
            current_ts,
            time_passed,
            time_remaining,
        );
        assert!(current_ts >= start);
        assert!(current_ts <= end);
    }

    #[test]
    fn should_validate_chain_version() {
        let edh_chain_version = CHAIN_VERSION;
        let result = validate_chain_version(edh_chain_version);
        assert!(result.is_ok());

        let edh_chain_version = CHAIN_VERSION + 1;
        let result = validate_chain_version(edh_chain_version);
        assert!(result.is_err());
    }
}
