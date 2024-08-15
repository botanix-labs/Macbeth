#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxzy/reth/issues/"
)]
#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! A [Consensus] implementation of Clique Proof of Authority (POA)
//! that authoritymatically seals blocks.
//!
//! The Mining task polls a [MiningMode], and will return a list of transactions that are ready to
//! be mined.
//!
//! These downloaders poll the miner, assemble the block, and return transactions that are ready to
//! be mined.

use pbft::PbftCommitmentCriteria;
use reth_consensus::{Consensus, ConsensusError, InvalidAggregatedPublicKeyError};
use reth_consensus_common::{
    utils::{unix_timestamp, validate_chain_version, validate_extra_data_header_authorities},
    validation::{self},
};

use reth_node_ethereum::EthEvmConfig;
use reth_primitives::{
    constants::nums_secp256k1_pk, header_ext::HeaderExt, Address, ChainSpec, Header, SealedBlock,
    SealedHeader, U256,
};

use std::{net::SocketAddr, sync::Arc};
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::{error, warn};

mod block_builder;
mod block_fetcher;
mod builder;
mod compressor;
mod dkg;
mod engine_util;
mod epoch_manager;
mod excecution_utils;
mod frost_task;
mod healthcheck_task;
mod pbft;
mod pbft_task;
mod signing;
mod sync;
mod task;
pub mod utils;
mod utxo_sync;
pub use builder::AuthorityConsensusBuilder;

/// Max EDH size, assuming max inputs spent are 100 and the only spends are keyspends
/// This was calulated with the following formula
/// version + optional_fields bitmask + signers pk + witness (vec of sigs) + blockhash +
/// utxo_commit + block_witness + agg_pk For specific details see [ExtraDataHeader]
pub const MAX_EDH_SIZE: usize = 8005;

/// Ethereum authority consensus
///
/// This consensus engine does basic checks as outlined in the execution specs.
#[derive(Clone, Debug)]
pub struct AuthorityConsensus {
    /// Configuration
    chain_spec: Arc<ChainSpec>,
}

impl AuthorityConsensus {
    /// Create a new instance of [AuthorityConsensus]
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl Consensus for AuthorityConsensus {
    fn validate_header(&self, header: &SealedHeader) -> Result<(), ConsensusError> {
        reth_consensus_common::validation::validate_header_standalone(header, &self.chain_spec)?;
        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        let leader_selection_window = self
            .chain_spec
            .leader_selection_window
            .expect("block times to be set for PoA consensus");
        reth_consensus_common::utils::validate_against_parent(
            parent.header().clone(),
            header.header().clone(),
            leader_selection_window,
        )?;
        // TODO(armins) this was removed do we still need it?
        // validation::validate_header_regarding_parent(parent, header, &self.chain_spec)?;
        Ok(())
    }

    fn validate_block(&self, block: &SealedBlock) -> Result<(), ConsensusError> {
        reth_consensus_common::validation::validate_block_standalone(block, &self.chain_spec)
    }

    fn validate_header_with_total_difficulty(
        &self,
        header: &Header,
        total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        reth_consensus_common::validation::validate_header_with_total_difficulty(
            header,
            total_difficulty,
        )?;
        Ok(())
    }

    /// Validate poa extra data header
    fn validate_extra_data_header(
        &self,
        header: &Header,
        authority_signers: &[secp256k1::PublicKey],
        genesis_authorities: &[secp256k1::PublicKey],
        aggregate_public_key: Option<&secp256k1::PublicKey>,
    ) -> Result<(), ConsensusError> {
        // Skip over genesis
        if header.number == 0 {
            return Ok(());
        }

        // there should alawys be an aggregate public key for poa
        if aggregate_public_key.is_none() {
            return Err(ConsensusError::InvalidAggregatedPublicKey(
                InvalidAggregatedPublicKeyError::MissingAggregatedPublicKey,
            ));
        }

        // Check total size of the extra data header
        if header.extra_data.len() > MAX_EDH_SIZE {
            return Err(ConsensusError::ExtraDataExceedsMax { len: MAX_EDH_SIZE });
        }

        // First run the basic validation
        validation::validate_header_extradata(header)?;

        // Attempt to deserialize the extra data header
        let edh = header.deserialize_extra_data_header().map_err(|e| {
            error!("Failed to deserialize extra data header: {:?}", e);
            ConsensusError::ExtraDataInvalid
        })?;

        validate_chain_version(edh.chain_version)?;

        validate_extra_data_header_authorities(header, genesis_authorities)?;

        // Past genesis NUMS point should never be used as the aggregated public key
        if edh.aggregated_public_key == nums_secp256k1_pk() {
            return Err(ConsensusError::InvalidAggregatedPublicKey(
                InvalidAggregatedPublicKeyError::NumsAggregatePublicKeyPastGenesis,
            ));
        }

        if edh.aggregated_public_key != *aggregate_public_key.unwrap() {
            return Err(ConsensusError::InvalidAggregatedPublicKey(
                InvalidAggregatedPublicKeyError::InvalidAggregatedPublicKey,
            ));
        }

        // Validate a quorum of authority signatures except during pbft
        let valid_sigs = header.check_authority_sig_add(authority_signers).map_err(|e| {
            error!("Failed to validate authority signature: {:?}", e);
            ConsensusError::InvalidAuthoritySignature
        })?;

        if valid_sigs < PbftCommitmentCriteria::min_commitments(authority_signers.len() as u16) {
            return Err(ConsensusError::MissingQuorumOfAuthoritySignatures(
                authority_signers.len() as u16,
                valid_sigs,
            ));
        }
        // TODO (armins) in the future this is where we would validate federation votes

        Ok(())
    }

    /// Validate poa block beneficiary
    fn validate_block_beneficiary(&self, header: &Header) -> Result<(), ConsensusError> {
        if header.beneficiary != Address::ZERO {
            return Err(ConsensusError::BlockBeneficiaryIsNotBurnAddress);
        }

        Ok(())
    }

    /// Validates PoA header standalone according to the authority consensus rules.
    fn validate_header_standalone(
        &self,
        header: &Header,
        authority_signers: &[secp256k1::PublicKey],
        genesis_authorities: &[secp256k1::PublicKey],
        aggregate_public_key: Option<&secp256k1::PublicKey>,
    ) -> Result<(), ConsensusError> {
        if aggregate_public_key.is_none() {
            return Err(ConsensusError::InvalidAggregatedPublicKey(
                InvalidAggregatedPublicKeyError::MissingAggregatedPublicKey,
            ));
        }

        // run the reth header validation rule
        let sealed_header = header.clone().seal_slow();
        reth_consensus_common::validation::validate_header_standalone(
            &sealed_header,
            &self.chain_spec,
        )?;

        // Validate EDH serialization and signature on block
        self.validate_extra_data_header(
            header,
            authority_signers,
            genesis_authorities,
            aggregate_public_key,
        )?;

        // Validate fee benificiary
        self.validate_block_beneficiary(header)?;

        // Validate signer is in turn
        // TODO just for simplicity lets pull block time from botanix testnet chainsepc
        let leader_selection_window = self
            .chain_spec
            .leader_selection_window
            .expect("block times to be set for PoA consensus");
        header
            .validate_inturn(authority_signers, leader_selection_window)
            .map_err(|_| ConsensusError::AuthorityNotInTurn)?;
        // Place a tigher limit on the timestamp
        let current_timestamp = unix_timestamp();
        header.validate_timestamp(current_timestamp).map_err(|_| {
            if header.timestamp > current_timestamp {
                ConsensusError::TimestampIsInFuture {
                    timestamp: header.timestamp,
                    present_timestamp: current_timestamp,
                }
            } else {
                ConsensusError::TimestampIsInPast {
                    timestamp: header.timestamp,
                    present_timestamp: current_timestamp,
                }
            }
        })?;

        Ok(())
    }

    /// Validate poa extra data header
    /// This function will validate the extra data header and check for a quorum of signatures
    /// from authorities memebers.
    fn validate_extra_data_header_single_signer(
        &self,
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

        if edh.aggregated_public_key == nums_secp256k1_pk() {
            return Err(ConsensusError::InvalidAggregatedPublicKey(
                InvalidAggregatedPublicKeyError::NumsAggregatePublicKeyPastGenesis,
            ));
        }

        // Validate the authority signature and signature came from one of the authorities
        header.validate_first_authority_signature(authority_signers).map_err(|e| {
            error!("Failed to validate authority signature: {:?}", e);
            ConsensusError::InvalidAuthoritySignature
        })?;

        // Validate fee benificiary
        self.validate_block_beneficiary(header)?;

        let leader_selection_window = self
            .chain_spec
            .leader_selection_window
            .expect("block times to be set for PoA consensus");

        // Validate signer is in turn
        header
            .validate_inturn(authority_signers, leader_selection_window)
            .map_err(|_| ConsensusError::AuthorityNotInTurn)?;
        // Place a tigher limit on the timestamp
        let current_timestamp = unix_timestamp();
        header.validate_timestamp(current_timestamp).map_err(|_| {
            ConsensusError::TimestampIsInFuture {
                timestamp: header.timestamp,
                present_timestamp: current_timestamp,
            }
        })?;

        Ok(())
    }
}

/// In memory storage
/// All this struct does is provide a rwlock wrapper around the storage inner
#[derive(Clone, Debug)]
pub(crate) struct Storage<EF, BF, DB> {
    pub(crate) inner: Arc<RwLock<StorageInner<EF, BF, DB>>>,
}

impl<EF, BF, DB> Storage<EF, BF, DB> {
    /// Create a new instance of the storage
    pub(crate) fn new(
        genesis_authorities: Vec<secp256k1::PublicKey>,
        authorities: Vec<secp256k1::PublicKey>,
        signer_index: usize,
        authority: secp256k1::PublicKey,
        btc_network: bitcoin::Network,
        aggregate_public_key: Option<secp256k1::PublicKey>,
        authority_socket_addresses: Vec<SocketAddr>,
        evm_config: EthEvmConfig,
        chain_spec: Arc<ChainSpec>,
        bitcoind_factory: BF,
        executor_factory: EF,
        client: DB,
    ) -> Self {
        let storage_inner = StorageInner {
            genesis_authorities,
            authorities,
            signer_index,
            authority,
            aggregate_public_key,
            btc_network,
            authority_socket_addresses,
            evm_config,
            chain_spec,
            bitcoind_factory,
            executor_factory,
            client,
        };

        Self { inner: Arc::new(RwLock::new(storage_inner)) }
    }

    /// Returns the write lock of the storage
    pub(crate) async fn write(&self) -> RwLockWriteGuard<'_, StorageInner<EF, BF, DB>> {
        self.inner.write().await
    }

    /// Returns the read lock of the storage
    pub(crate) async fn read(&self) -> RwLockReadGuard<'_, StorageInner<EF, BF, DB>> {
        self.inner.read().await
    }
}

#[derive(Debug)]
/// In-memory storage for the chain the authority seal engine is building.
/// data shared amongst the different tasks should be stored here and protected by a rwlock
pub(crate) struct StorageInner<EF, BF, DB> {
    /// The authority list in the genesis block
    pub(crate) genesis_authorities: Vec<secp256k1::PublicKey>,
    /// Keep track of the signers
    /// This value is pulled from the latest epoch block EDH
    /// and should be the same as genesis_authorities as long as the federation is static
    pub(crate) authorities: Vec<secp256k1::PublicKey>,
    /// keep track of my place among the signer
    /// This will change as new signers are removed
    pub(crate) signer_index: usize,
    /// Authority Signer public key
    pub(crate) authority: secp256k1::PublicKey,
    /// The aggregate public key of the FROST threshold signature scheme
    /// Should get populated after DKG
    pub(crate) aggregate_public_key: Option<secp256k1::PublicKey>,
    /// Bitcoin network
    btc_network: bitcoin::Network,
    /// Authority socket addresses pulled from federation config
    authority_socket_addresses: Vec<SocketAddr>,
    /// Evm config
    evm_config: EthEvmConfig,
    /// Bitcoind Factory
    bitcoind_factory: BF,
    /// Chain spec
    chain_spec: Arc<ChainSpec>,
    /// Executor Factory
    executor_factory: EF,
    /// The db provider
    client: DB,
}

impl<EF, BF, DB> StorageInner<EF, BF, DB> {
    fn get_authorities(&self) -> Vec<secp256k1::PublicKey> {
        self.authorities.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use reth_consensus::InvalidAggregatedPublicKeyError;
    use reth_consensus_common::utils::{
        block_fees_split, current_inturn_index, get_block_producer_address, get_in_turn_interval,
        is_inturn, validate_against_parent,
    };
    use reth_primitives::{
        extra_data_header::ExtraDataHeader, public_key_to_address, Bytes, BOTANIX_TESTNET,
    };

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
        let consensus = AuthorityConsensus::new(Arc::new(BOTANIX_TESTNET.as_ref().to_owned()));
        let mut header = Header::default();
        header.number = 0;
        let authority_signers = vec![];
        // Just use the first key as the dummy agg key
        let sk1 = secp256k1::SecretKey::from_str(SK1).unwrap();
        let dummy_agg_key = sk1.public_key(secp256k1::SECP256K1);

        let result = consensus.validate_extra_data_header(
            &header,
            &authority_signers,
            &authority_signers,
            Some(&dummy_agg_key),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn fails_when_edh_exceeds_max_size() {
        let consensus = AuthorityConsensus::new(Arc::new(BOTANIX_TESTNET.as_ref().to_owned()));
        // In this case we are signing with a non federation different key
        let mut edh = ExtraDataHeader::default();
        let sk1 = bitcoin::secp256k1::SecretKey::from_str(SK1).unwrap();
        let msg = [0u8; 64];
        let mut wit = bitcoin::witness::Witness::default();
        wit.push(msg.clone());
        let mut witnesses = vec![];
        for _ in 0..1000 {
            witnesses.push(wit.clone());
        }
        edh.witness_data = Some(witnesses);
        edh.set_optional_fields_bitmask();

        // Just use the first key as the dummy agg key
        let dummy_agg_key = sk1.public_key(secp256k1::SECP256K1);
        edh.aggregated_public_key = dummy_agg_key;

        let authority_signers = vec![sk1.public_key(secp256k1::SECP256K1)];
        let mut header = Header::default();
        header.number = 1;
        header.extra_data = Bytes::from(edh.serialize());
        header.sign_block(&sk1).expect("valid sign");

        let result = consensus.validate_extra_data_header(
            &header,
            &authority_signers,
            &authority_signers,
            Some(&dummy_agg_key),
        );
        assert!(result.is_err());
        assert_eq!(
            result.err().unwrap(),
            ConsensusError::ExtraDataExceedsMax { len: MAX_EDH_SIZE }
        );
    }

    #[test]
    fn should_fail_on_invalid_signature() {
        let consensus = AuthorityConsensus::new(Arc::new(BOTANIX_TESTNET.as_ref().to_owned()));
        // In this case we are signing with a non federation different key
        let mut edh = ExtraDataHeader::default();
        let sk1 = secp256k1::SecretKey::from_str(SK1).unwrap();
        let non_fed = secp256k1::SecretKey::from_str(
            "1bc1f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019",
        )
        .unwrap();

        // Just use the first key as the dummy agg key
        let dummy_agg_key = sk1.public_key(secp256k1::SECP256K1);
        edh.aggregated_public_key = dummy_agg_key;

        let authority_signers = vec![sk1.public_key(secp256k1::SECP256K1)];
        let mut header = Header::default();

        header.number = 1;
        header.extra_data = Bytes::from(edh.serialize());
        header.sign_block(&non_fed).expect("valid sign");

        let result = consensus.validate_extra_data_header(
            &header,
            &authority_signers,
            &authority_signers,
            Some(&dummy_agg_key),
        );
        assert!(result.is_err());

        // reset header and try again with a
        let mut header = Header::default();
        header.number = 1;
        header.extra_data = Bytes::from(edh.serialize());
        header.sign_block(&sk1).expect("valid sign");

        let result = consensus.validate_extra_data_header(
            &header,
            &authority_signers,
            &authority_signers,
            Some(&dummy_agg_key),
        );
        assert!(result.is_ok())
    }

    #[test]
    fn should_not_accept_edh_with_nums_point() {
        let consensus = AuthorityConsensus::new(Arc::new(BOTANIX_TESTNET.as_ref().to_owned()));
        // By default edh will use the nums point
        let edh = ExtraDataHeader::default();

        // Just use the first key as the dummy agg key
        let sk1 = secp256k1::SecretKey::from_str(SK1).unwrap();
        let dummy_agg_key = sk1.public_key(secp256k1::SECP256K1);

        let sk1 = secp256k1::SecretKey::from_str(SK1).unwrap();
        let authority_signers = vec![sk1.public_key(secp256k1::SECP256K1)];
        let mut header = Header::default();
        header.number = 1;
        header.extra_data = Bytes::from(edh.serialize());
        header.sign_block(&sk1).expect("valid sign");

        let result = consensus.validate_extra_data_header(
            &header,
            &authority_signers,
            &authority_signers,
            Some(&dummy_agg_key),
        );
        assert_eq!(
            result.err().unwrap(),
            ConsensusError::InvalidAggregatedPublicKey(
                InvalidAggregatedPublicKeyError::NumsAggregatePublicKeyPastGenesis
            )
        );
    }

    #[test]
    fn should_not_accept_edh_with_invalid_agg_pk() {
        let consensus = AuthorityConsensus::new(Arc::new(BOTANIX_TESTNET.as_ref().to_owned()));
        // By default edh will use the nums point
        let mut edh = ExtraDataHeader::default();

        // Just use the first key as the dummy agg key
        let sk1 = secp256k1::SecretKey::from_str(SK1).unwrap();
        let dummy_agg_key = sk1.public_key(secp256k1::SECP256K1);

        edh.aggregated_public_key = dummy_agg_key;

        let different_key = secp256k1::SecretKey::from_str(SK2).unwrap();
        let different_pk = different_key.public_key(secp256k1::SECP256K1);

        let sk1 = secp256k1::SecretKey::from_str(SK1).unwrap();
        let authority_signers = vec![sk1.public_key(secp256k1::SECP256K1)];
        let mut header = Header::default();
        header.number = 1;
        header.extra_data = Bytes::from(edh.serialize());
        header.sign_block(&sk1).expect("valid sign");

        let result = consensus.validate_extra_data_header(
            &header,
            &authority_signers,
            &authority_signers,
            Some(&different_pk),
        );
        assert_eq!(
            result.err().unwrap(),
            ConsensusError::InvalidAggregatedPublicKey(
                InvalidAggregatedPublicKeyError::InvalidAggregatedPublicKey
            )
        );
    }

    #[test]
    fn unix_timestamp() {
        let timestamp = super::unix_timestamp();
        assert!(timestamp > 0);
    }

    #[test]
    fn should_validate_poa_block_beneficiary() {
        // default beneficiary is the burn address
        let consensus = AuthorityConsensus::new(Arc::new(BOTANIX_TESTNET.as_ref().to_owned()));
        let header = Header::default();
        let result = consensus.validate_block_beneficiary(&header);
        assert!(result.is_ok());
    }

    #[test]
    fn should_fail_validate_poa_block_beneficiary() {
        let consensus = AuthorityConsensus::new(Arc::new(BOTANIX_TESTNET.as_ref().to_owned()));
        let mut header = Header::default();
        header.beneficiary =
            Address::from_str("0x4e0f6e05C8ca4b3dc2B7b7Ad6249B149b1980394").unwrap();
        let result = consensus.validate_block_beneficiary(&header);
        assert!(result.is_err());
    }

    #[test]
    fn validate_against_parent_skip_gensis() {
        let mut parent = Header::default();
        parent.number = 0;
        let current = Header::default();
        let result = validate_against_parent(parent, current, 5);
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

        let result = validate_against_parent(parent, current, 5);
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

        let result = validate_against_parent(parent, current, 5);
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

        let result = validate_against_parent(parent, current, 5);
        assert!(result.is_ok());
    }

    #[test]
    fn is_inturn_true() {
        let authorities_len = 1;
        let signer_index = 0;
        assert!(is_inturn(authorities_len, signer_index, 5));
    }

    #[test]
    fn is_inturn_false() {
        let authorities_len = 1;
        let signer_index = 1;
        assert!(!is_inturn(authorities_len, signer_index, 5));
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

    #[test]
    fn get_inturn_interval_secs_based() {
        let current_ts = super::unix_timestamp();
        let authorities_len = 10;
        let current_in_turn_signer = current_inturn_index(authorities_len, current_ts, 5);
        let (start, end, time_passed, time_remaining) =
            get_in_turn_interval(authorities_len, current_in_turn_signer, current_ts, 5);

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
}
