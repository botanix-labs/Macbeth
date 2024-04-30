use crate::validation;
use reth_botanix_lib::header_ext::{GetAuthoritiesError, HeaderExt, RecoverAuthorityError};
use reth_interfaces::{blockchain_tree::BlockchainTreeEngine, consensus::ConsensusError};
use reth_primitives::{
    constants::STAKING_CONTRACT_ADDRESS, keccak256, public_key_to_address, Address,
    ChainSpec, Header, U256,
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

/// Returns the authority signer index
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
    let binding = header.recovered_signed_authorities().expect("recovered authority");
    let block_builder_public_key = binding.get(0).expect("block producer authority to be present");
    public_key_to_address(*block_builder_public_key)
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

/// Validate poa extra data header
/// This function will validate the extra data header and check for a quorum of signatures
/// from authorities memebers.
/// TODO (armins) validate only 2/3 of the authorities have signed, rn we are checking for n
pub fn validate_poa_extra_data_header(
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
    let edh = header.deserialize_extra_data_header().map_err(|e| {
        error!("Failed to deserialize extra data header: {:?}", e);
        ConsensusError::ExtraDataInvalid
    })?;
    // Validate the authority signature and signature came from one of the authorities
    let sig_hash = header.create_sighash().map_err(|e| {
        error!("Failed to deserialize extra data header: {:?}", e);
        ConsensusError::ExtraDataInvalid
    })?;
    let valid_sigs =
        edh.check_authority_sig_add(&sig_hash.to_vec(), authority_signers).map_err(|e| {
            error!("Failed to validate authority signature: {:?}", e);
            ConsensusError::InvalidAuthoritySignature
        })?;

    if valid_sigs != authority_signers.len() as u16 {
        return Err(ConsensusError::MissingQuorumOfAuthoritySignatures(
            authority_signers.len() as u16,
            valid_sigs,
        ));
    }
    // TODO (armins) in the future this is where we would validate federation votes

    Ok(())
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
    let edh = header.deserialize_extra_data_header().map_err(|e| {
        error!("Failed to deserialize extra data header: {:?}", e);
        ConsensusError::ExtraDataInvalid
    })?;
    // Validate the authority signature and signature came from one of the authorities
    let sig_hash = header.create_sighash().map_err(|e| {
        error!("Failed to deserialize extra data header: {:?}", e);
        ConsensusError::ExtraDataInvalid
    })?;
    edh.validate_first_authority_signature(&sig_hash.to_vec(), authority_signers).map_err(|e| {
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
    // use minutes as time unit to determine in turn
    let timestamp = unix_timestamp() / 60;

    (timestamp / authorities_len) % authorities_len == signer_index
}

/// Returns the index of the authority which is currently in turn
pub fn current_inturn_index(authorities_len: u64) -> u64 {
    // use minutes as time unit to determine in turn
    let timestamp = unix_timestamp() / 60;
    (timestamp / authorities_len) % authorities_len
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use reth_botanix_lib::{extra_data_header::ExtraDataHeader, header_ext::HeaderExt};

    use super::*;

    #[allow(dead_code)]
    const EDH_DEFAULT_SIGHASH: &str =
        "0xaaa3492fe3eec8da1ca35aca5930a44b1a5805e813bdd1773678b5041d905276";

    #[allow(dead_code)]
    const SK1: &str = "1aabc5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019";
    #[allow(dead_code)]
    const SK2: &str = "1bc1f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019";

    #[allow(dead_code)]
    fn generate_secret_key(hex_string: &str) -> secp256k1::SecretKey {
        secp256k1::SecretKey::from_str(hex_string).expect("Invalid hex string for SecretKey")
    }
    #[allow(dead_code)]
    fn sign_block_helper(header: &mut Header, signer_sk: Option<&str>) {
        let mut edh = ExtraDataHeader::default();
        let sk1 = generate_secret_key(SK1);
        let sk2 = generate_secret_key(SK2);

        edh.authority_signers = Some(vec![
            secp256k1::PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), &sk1),
            secp256k1::PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), &sk2),
        ]);
        edh.set_optional_fields_bitmask();
        header.extra_data = Bytes::from(edh.serialize());

        header.number = 1;
        if let Some(sk_str) = signer_sk {
            let sk = generate_secret_key(sk_str);
            header.sign_block(&sk).unwrap();
        } else {
            header.sign_block(&sk1).unwrap();
        }
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
        let edh = ExtraDataHeader::default();
        let sk1 = secp256k1::SecretKey::from_str(SK1).unwrap();
        let non_fed = secp256k1::SecretKey::from_str(
            "1bc1f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019",
        )
        .unwrap();

        let authority_signers = vec![sk1.public_key(secp256k1::SECP256K1)];
        let mut header = Header::default();
        header.number = 1;
        header.extra_data = Bytes::from(edh.serialize());
        header.sign_block(&non_fed).expect("valid sign");

        let result = validate_poa_extra_data_header(&header, &authority_signers);
        assert!(result.is_err());

        // reset header and try again with a
        let mut header = Header::default();
        header.number = 1;
        header.extra_data = Bytes::from(edh.serialize());
        header.sign_block(&sk1).expect("valid sign");

        let result = validate_poa_extra_data_header(&header, &authority_signers);
        assert!(result.is_ok())
    }

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
        parent.timestamp = 1704834442_u64;
        current.number = 2;
        current.timestamp = 1704834442_u64 + 60;

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

    #[test]
    fn is_inturn_true() {
        let authorities_len = 1;
        let signer_index = 0;
        assert!(is_inturn(authorities_len, signer_index));
    }

    #[test]
    fn is_inturn_false() {
        let authorities_len = 1;
        let signer_index = 1;
        assert!(!is_inturn(authorities_len, signer_index));
    }

    #[test]
    fn should_split_rewards() {
        let base_block_reward = 100;
        let (botanix_reward, beneficiary_reward) = block_fees_split(base_block_reward);
        assert_eq!(botanix_reward, 20);
        assert_eq!(beneficiary_reward, 80);
    }

    #[test]
    fn should_get_block_producer_address_from_header() {
        let mut header = Header::default();
        sign_block_helper(&mut header, None);
        let edh = ExtraDataHeader::deserialize(&mut header.extra_data.to_vec().as_slice()).unwrap();
        let block_producer_address = get_block_producer_address(&header);
        assert_eq!(
            block_producer_address,
            public_key_to_address(edh.authority_signers.unwrap()[0])
        );
    }
}
