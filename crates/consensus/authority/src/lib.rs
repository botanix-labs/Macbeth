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

use reth_chainspec::ChainSpec;
use reth_consensus::{Consensus, ConsensusError, InvalidAggregatedPublicKeyError, PostExecutionInput};
use reth_consensus_common::{
    utils::validate_chain_version,
    validation::{self},
};

use reth_node_ethereum::EthEvmConfig;
use reth_primitives::{
    constants::nums_secp256k1_pk, header_ext::HeaderExt, Address, Header, SealedBlock,
    SealedHeader, U256,
};

use reth_primitives::BlockWithSenders;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::{error, warn};

mod block_fetcher;
mod builder;
mod comet_bft;

pub use comet_bft::light_client::LightCBFTClientBuilder;
mod compressor;
mod dkg;
mod engine_util;
mod excecution_utils;
mod frost_task;
mod healthcheck_task;
mod signing;
mod sync;
pub mod utils;
mod utxo_sync;
pub use builder::AuthorityConsensusBuilder;

/// Max EDH size, assuming max inputs spent are 1000 and the only spends are keyspends
/// This was calulated with the following formula
/// version + optional_fields bitmask + signers pk + witness (vec of sigs) + blockhash +
/// utxo_commit + block_witness + agg_pk For specific details see [ExtraDataHeader]
pub const MAX_EDH_SIZE: usize = 80050;

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
    fn validate_block_pre_execution(&self, block: &SealedBlock) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_block_post_execution(
        &self,
        block: &BlockWithSenders,
        input: PostExecutionInput<'_>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_header(&self, header: &SealedHeader) -> Result<(), ConsensusError> {
        // reth_consensus_common::validation::validate_header_standalone(header, &self.chain_spec)?;
        // //TODO check this
        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        header.validate_against_parent(parent, &self.chain_spec).map_err(ConsensusError::from)?;
        Ok(())
    }

    fn validate_block(&self, block: &SealedBlock) -> Result<(), ConsensusError> {
        //reth_consensus_common::validation::validate_block_standalone(block, &self.chain_spec) //
        // TODO: check this
        Ok(())
    }

    fn validate_header_with_total_difficulty(
        &self,
        header: &Header,
        total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        // reth_consensus_common::validation::validate_header_with_total_difficulty(
        //     header,
        //     total_difficulty,
        // )?;
        // TODO: check this
        Ok(())
    }

    /// Validate poa extra data header
    fn validate_extra_data_header(
        &self,
        header: &Header,
        _genesis_authorities: &[secp256k1::PublicKey],
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

        // TODO this needs to be re-enabled to check for CBFT block signatures
        // Validate a quorum of authority signatures except during pbft
        // let valid_sigs = header.check_authority_sig_add(authority_signers).map_err(|e| {
        //     error!("Failed to validate authority signature: {:?}", e);
        //     ConsensusError::InvalidAuthoritySignature
        // })?;

        // if valid_sigs < PbftCommitmentCriteria::min_commitments(authority_signers.len() as u16) {
        //     return Err(ConsensusError::MissingQuorumOfAuthoritySignatures(
        //         authority_signers.len() as u16,
        //         valid_sigs,
        //     ));
        // }

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
        // reth_consensus_common::validation::validate_header_standalone(
        //     &sealed_header,
        //     &self.chain_spec,
        // )?;
        // TODO: check this

        // Validate EDH serialization and signature on block
        self.validate_extra_data_header(header, genesis_authorities, aggregate_public_key)?;

        // Validate fee benificiary
        self.validate_block_beneficiary(header)?;

        Ok(())
    }
}

/// In memory storage
/// All this struct does is provide a rwlock wrapper around the storage inner
#[derive(Clone, Debug)]
pub(crate) struct Storage<EF, BF, DB> {
    pub(crate) client: DB,
    /// The authority list in the genesis block
    pub(crate) genesis_authorities: Vec<secp256k1::PublicKey>,
    /// keep track of my place among the signer
    /// This will change as new signers are removed
    pub(crate) signer_index: usize,
    /// Authority Signer public key
    pub(crate) authority: secp256k1::PublicKey,
    /// Bitcoin network
    pub(crate) btc_network: bitcoin::Network,
    /// Authority socket addresses pulled from federation config
    pub(crate) authority_socket_addresses: Vec<SocketAddr>,
    /// Evm config
    pub(crate) evm_config: EthEvmConfig,
    /// Bitcoind Factory
    pub(crate) bitcoind_factory: BF,
    /// Chain spec
    pub(crate) chain_spec: Arc<ChainSpec>,
    /// Executor Factory
    pub(crate) executor_factory: EF,
    // The inner storage, everything here is rw locked
    pub(crate) inner: Arc<RwLock<StorageInner>>,
}

impl<EF, BF, DB: Clone> Storage<EF, BF, DB> {
    /// Create a new instance of the storage
    pub(crate) fn new(
        genesis_authorities: Vec<secp256k1::PublicKey>,
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
        let storage_inner = StorageInner { aggregate_public_key };

        Self {
            client,
            genesis_authorities,
            signer_index,
            authority,
            btc_network,
            authority_socket_addresses,
            evm_config,
            chain_spec,
            bitcoind_factory,
            executor_factory,
            inner: Arc::new(RwLock::new(storage_inner)),
        }
    }

    /// Returns the write lock of the storage
    pub(crate) async fn write(&self) -> RwLockWriteGuard<'_, StorageInner> {
        self.inner.write().await
    }

    /// Returns the read lock of the storage
    pub(crate) async fn read(&self) -> RwLockReadGuard<'_, StorageInner> {
        self.inner.read().await
    }
}

#[derive(Debug)]
/// In-memory storage for the chain the authority seal engine is building.
/// data shared amongst the different tasks should be stored here and protected by a rwlock
pub(crate) struct StorageInner {
    /// The aggregate public key of the FROST threshold signature scheme
    /// Should get populated after DKG
    pub(crate) aggregate_public_key: Option<secp256k1::PublicKey>,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use reth_chainspec::BOTANIX_TESTNET;
    use reth_consensus::InvalidAggregatedPublicKeyError;
    use reth_consensus_common::utils::{
        block_fees_split, current_inturn_index, get_block_producer_address, get_in_turn_interval,
        is_inturn,
    };
    use reth_primitives::{extra_data_header::ExtraDataHeader, public_key_to_address, Bytes};

    use super::*;

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

        let result =
            consensus.validate_extra_data_header(&header, &authority_signers, Some(&dummy_agg_key));

        assert!(result.is_ok());
    }

    #[test]
    fn fails_when_edh_exceeds_max_size() {
        let consensus = AuthorityConsensus::new(Arc::new(BOTANIX_TESTNET.as_ref().to_owned()));
        // In this case we are signing with a non federation different key
        let mut edh = ExtraDataHeader::default();
        let sk1 = secp256k1::SecretKey::from_str(SK1).unwrap();
        let msg = [0u8; 64];
        let mut wit = bitcoin::witness::Witness::default();
        wit.push(msg.clone());
        let mut witnesses = vec![];
        // MAX_EDH_SIZE is 80050 which is ~1211 witnesses
        for _ in 0..1211 {
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

        let result =
            consensus.validate_extra_data_header(&header, &authority_signers, Some(&dummy_agg_key));
        assert!(result.is_err());
        assert_eq!(
            result.err().unwrap(),
            ConsensusError::ExtraDataExceedsMax { len: MAX_EDH_SIZE }
        );
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

        let result =
            consensus.validate_extra_data_header(&header, &authority_signers, Some(&dummy_agg_key));
        assert_eq!(
            result.err().unwrap(),
            ConsensusError::InvalidAggregatedPublicKey(
                InvalidAggregatedPublicKeyError::NumsAggregatePublicKeyPastGenesis
            )
        );
    }

    // TODO add this back in 
    // #[test]
    // fn should_not_accept_edh_with_invalid_agg_pk() {
    //     let consensus = AuthorityConsensus::new(Arc::new(BOTANIX_TESTNET.as_ref().to_owned()));
    //     // By default edh will use the nums point
    //     let mut edh = ExtraDataHeader::default();

    //     // Just use the first key as the dummy agg key
    //     let sk1 = secp256k1::SecretKey::from_str(SK1).unwrap();
    //     let dummy_agg_key = sk1.public_key(secp256k1::SECP256K1);

    //     edh.aggregated_public_key = dummy_agg_key;

    //     let different_key = secp256k1::SecretKey::from_str(SK2).unwrap();
    //     let different_pk = different_key.public_key(secp256k1::SECP256K1);

    //     let sk1 = secp256k1::SecretKey::from_str(SK1).unwrap();
    //     let authority_signers = vec![sk1.public_key(secp256k1::SECP256K1)];
    //     let mut header = Header::default();
    //     header.number = 1;
    //     header.extra_data = Bytes::from(edh.serialize());
    //     header.sign_block(&sk1).expect("valid sign");

    //     let result =
    //         consensus.validate_extra_data_header(&header, &authority_signers, Some(&different_pk));
    //     assert_eq!(
    //         result.err().unwrap(),
    //         ConsensusError::InvalidAggregatedPublicKey(
    //             InvalidAggregatedPublicKeyError::InvalidAggregatedPublicKey
    //         )
    //     );
    // }

    #[test]
    fn unix_timestamp() {
        let timestamp = reth_consensus_common::utils::unix_timestamp();
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
    fn should_split_rewards() {
        let base_block_reward = 100;
        let (botanix_reward, beneficiary_reward) = block_fees_split(base_block_reward);
        assert_eq!(botanix_reward, 20);
        assert_eq!(beneficiary_reward, 80);
    }

    #[test]
    fn get_inturn_interval_secs_based() {
        let current_ts = reth_consensus_common::utils::unix_timestamp();
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
