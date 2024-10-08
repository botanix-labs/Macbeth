use reth_consensus::ConsensusError;

use reth_primitives::{
    extra_data_header::CHAIN_VERSION, revm_primitives::FixedBytes, Address, Header, U256,
};

use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the unix timestamp in seconds
pub fn unix_timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
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

/// Check the extra data header field has the current chain version
pub fn validate_chain_version(edh_chain_version: u32) -> Result<(), ConsensusError> {
    if edh_chain_version != CHAIN_VERSION {
        return Err(ConsensusError::InvalidChainVersion);
    }

    Ok(())
}

/// Convert FixedBytes<32> to U256
pub fn fixed_bytes_32_to_u256(value: FixedBytes<32>) -> U256 {
    let mut value_array = [0u8; 32];
    value_array.copy_from_slice(value.as_slice());
    U256::from_le_bytes(value_array)
}

/// Returns true if the authority is in turn
pub fn is_inturn(
    authorities_len: u64,
    signer_index: u64,
    time_range: u64,
    random_source: FixedBytes<32>,
) -> bool {
    // convert types to U256 since random_source is 32 bytes and do arithmetic
    let authorities_len_u256 = U256::from(authorities_len);
    let signer_index_u256 = U256::from(signer_index);
    let time_range_u256 = U256::from(time_range);
    let random_source_u256 = fixed_bytes_32_to_u256(random_source);

    let cycle_length = authorities_len_u256 * time_range_u256; // Full cycle length in seconds

    // Calculate the position in the current cycle
    let position_in_cycle = random_source_u256 % cycle_length;

    // Determine the current signer index based on the position in the cycle
    // Each signer's turn lasts for `block_time` seconds
    (position_in_cycle / time_range_u256) % authorities_len_u256 == signer_index_u256
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
    fn should_validate_chain_version() {
        let edh_chain_version = CHAIN_VERSION;
        let result = validate_chain_version(edh_chain_version);
        assert!(result.is_ok());

        let edh_chain_version = CHAIN_VERSION + 1;
        let result = validate_chain_version(edh_chain_version);
        assert!(result.is_err());
    }

    #[test]
    fn should_convert_fixed_bytes_32_to_u256() {
        let value = FixedBytes::from([0u8; 32]);
        let u256 = fixed_bytes_32_to_u256(value);
        assert_eq!(u256, U256::ZERO);
    }
}
