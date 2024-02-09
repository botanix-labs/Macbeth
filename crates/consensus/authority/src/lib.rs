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
use reth_botanix_lib::extra_data_header::ExtraDataHeader;
use reth_consensus_common::{
    utils::unix_timestamp,
    validation::{self, validate_poa_header_standalone},
};
use reth_interfaces::{
    consensus::{Consensus, ConsensusError},
    executor::{BlockExecutionError, BlockValidationError},
};
use reth_node_api::{evm, ConfigureEvmEnv, EngineTypes};
use reth_primitives::{
    constants::{EMPTY_RECEIPTS, EMPTY_TRANSACTIONS, ETHEREUM_BLOCK_GAS_LIMIT},
    proofs, public_key_to_address,
    revm_primitives::FixedBytes,
    Address, Block, BlockBody, BlockHash, BlockHashOrNumber, BlockWithSenders, Bloom, Bytes,
    ChainSpec, Header, ReceiptWithBloom, SealedBlock, SealedHeader, TransactionSigned,
    EMPTY_OMMER_ROOT_HASH, U256,
};
use reth_provider::{
    BlockExecutor, BlockReaderIdExt, BundleStateWithReceipts, CanonChainTracker,
    StateProviderFactory,
};
use reth_revm::{
    database::StateProviderDatabase, db::states::bundle_state::BundleRetention,
    processor::EVMProcessor, State,
};
use std::{clone, sync::Arc};
use voting::{AuthorityVoteCollection, Vote};

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use tracing::{trace, warn};
mod block_builder;
mod block_fetcher;
mod builder;
mod engine_util;
mod epoch_manager;
mod sync;
mod task;
mod utils;
mod voting;

pub use builder::AuthorityConsensusBuilder;

/// Ethereum authority consensus
///
/// This consensus engine does basic checks as outlined in the execution specs.
#[derive(Debug)]
pub struct AuthorityConsensus {
    /// Configuration
    chain_spec: Arc<ChainSpec>,
}

impl AuthorityConsensus {
    /// Create a new instance of [AuthorityConsensus]
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        // TODO(armins) most likely we need to pass storage here
        Self { chain_spec }
    }
}

impl Consensus for AuthorityConsensus {
    fn validate_header(&self, header: &SealedHeader) -> Result<(), ConsensusError> {
        validation::validate_header_standalone(header, &self.chain_spec)?;
        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        reth_consensus_common::utils::validate_against_parent(
            parent.header().clone(),
            header.header().clone(),
        )?;
        // TODO(armins) this was removed do we still need it?
        // validation::validate_header_regarding_parent(parent, header, &self.chain_spec)?;
        Ok(())
    }

    fn validate_block(&self, block: &SealedBlock) -> Result<(), ConsensusError> {
        validation::validate_block_standalone(block, &self.chain_spec)
    }

    fn validate_header_with_total_difficulty(
        &self,
        header: &Header,
        total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        validation::validate_header_with_total_difficulty(header, total_difficulty)?;
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum StorageCreationError {
    /// empty headers
    EmptyHeaders,
}

/// In memory storage
#[derive(Clone, Debug)]
pub(crate) struct Storage<Client> {
    pub(crate) inner: Arc<RwLock<StorageInner<Client>>>,
}

// == impl Storage ===
impl<Client> Storage<Client>
where
    Client: BlockReaderIdExt + StateProviderFactory + CanonChainTracker + Clone + 'static,
{
    fn try_new(
        client: Client,
        headers: &mut Vec<SealedHeader>,
        authorities: Vec<secp256k1::PublicKey>,
        signer_index: usize,
        pk: secp256k1::PublicKey,
    ) -> Result<Self, StorageCreationError> {
        if headers.len() == 0 {
            return Err(StorageCreationError::EmptyHeaders);
        }
        // sort the headers by block numbers
        headers.sort_by(|a, b| a.number.cmp(&b.number));

        let storage = StorageInner {
            client: client.clone(),
            authorities,
            signer_index,
            authority: pk,
            authority_votes: AuthorityVoteCollection::default(),
        };

        Ok(Self { inner: Arc::new(RwLock::new(storage)) })
    }

    /// Returns the write lock of the storage
    pub(crate) async fn write(&self) -> RwLockWriteGuard<'_, StorageInner<Client>> {
        self.inner.write().await
    }

    #[allow(dead_code)]
    /// Returns the read lock of the storage
    pub(crate) async fn read(&self) -> RwLockReadGuard<'_, StorageInner<Client>> {
        self.inner.read().await
    }
}

#[derive(Debug)]
/// In-memory storage for the chain the authority seal engine is building.
pub(crate) struct StorageInner<Client> {
    client: Client,
    /// Keep track of current votes
    pub(crate) authority_votes: AuthorityVoteCollection,
    /// Keep track of the  signers
    pub(crate) authorities: Vec<secp256k1::PublicKey>,
    /// keep track of my place among the singer
    /// This will change as new signers are removed
    pub(crate) signer_index: usize,
    /// Authority Signer public key
    pub(crate) authority: secp256k1::PublicKey,
}

// === impl StorageInner ===

impl<Client> StorageInner<Client>
where
    Client: BlockReaderIdExt + StateProviderFactory + CanonChainTracker + Clone + 'static,
{
    /// Returns the block hash for the given block number if it exists.
    #[allow(dead_code)]
    pub(crate) fn block_hash(&self, num: u64) -> Option<BlockHash> {
        self.client.block_hash(num).ok().flatten()
    }

    pub(crate) fn get_best_block_and_hash(
        &self,
    ) -> Result<(u64, FixedBytes<32>), BlockExecutionError> {
        let best_block =
            self.client.best_block_number().map_err(|_| BlockExecutionError::ProviderError)?;

        let best_hash = self
            .client
            .block_hash(best_block)
            .map_err(|_| BlockExecutionError::ProviderError)?
            .unwrap_or_else(|| {
                panic!("{}", format!("Missing block hash for best block {:?}", best_block))
            });

        Ok((best_block, best_hash))
    }

    /// Fills in pre-execution header fields based on the current best block and given
    /// transactions.
    pub(crate) fn build_header_template(
        &self,
        transactions: &Vec<TransactionSigned>,
        chain_spec: &Arc<ChainSpec>,
        vote: &Option<(secp256k1::PublicKey, Vote)>,
        sk: &secp256k1::SecretKey,
        secp: &secp256k1::Secp256k1<secp256k1::All>,
    ) -> Result<Header, BlockExecutionError> {
        let (best_block, best_hash) = self.get_best_block_and_hash()?;
        let timestamp = unix_timestamp();

        // check previous block for base fee
        let base_fee_per_gas = self
            .client
            .header_by_hash_or_number(BlockHashOrNumber::Number(best_block))
            .expect("header to exist")
            .and_then(|parent| parent.next_block_base_fee(chain_spec.base_fee_params(timestamp)));

        // derive beneficary address being the producuing block federation member address
        let beneficiary_pub_key = secp256k1::PublicKey::from_secret_key(secp, sk);
        let beneficiary_address = public_key_to_address(beneficiary_pub_key);

        let mut header = Header {
            parent_hash: best_hash,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: beneficiary_address,
            state_root: Default::default(),
            transactions_root: Default::default(),
            receipts_root: Default::default(),
            withdrawals_root: None,
            logs_bloom: Default::default(),
            difficulty: Default::default(),
            number: best_block + 1,
            gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            gas_used: 0,
            timestamp,
            mix_hash: Default::default(),
            nonce: 0,
            base_fee_per_gas,
            blob_gas_used: None,
            excess_blob_gas: None,
            extra_data: Default::default(),
            parent_beacon_block_root: None,
        };

        // Add the vote to the header using the nonce field
        if let Some(vote) = vote {
            header.nonce = vote.1 as u64;
        }

        header.transactions_root = if transactions.is_empty() {
            EMPTY_TRANSACTIONS
        } else {
            proofs::calculate_transaction_root(transactions)
        };

        Ok(header)
    }

    /// Executes the block with the given block and senders, on the provided [Executor].
    ///
    /// This returns the poststate from execution and post-block changes, as well as the gas used.
    pub(crate) fn execute<EvmConfig>(
        &mut self,
        block: &BlockWithSenders,
        executor: &mut EVMProcessor<'_, EvmConfig>,
        senders: Vec<Address>,
        recent_block_header: Option<(bitcoin::blockdata::block::Header, u32)>,
    ) -> Result<(BundleStateWithReceipts, u64), BlockExecutionError>
    where
        EvmConfig: ConfigureEvmEnv + Clone + 'static,
    {
        // set the first block to find the correct index in bundle state
        executor.set_first_block(block.number);

        let (receipts, gas_used) =
            executor.execute_transactions(block, U256::ZERO, recent_block_header)?;

        // Save receipts.
        executor.save_receipts(receipts)?;

        // add post execution state change
        // Withdrawals, rewards etc.
        executor.apply_post_execution_state_change(block, U256::ZERO)?;

        // merge transitions
        executor.db_mut().merge_transitions(BundleRetention::Reverts);

        // apply post block changes
        Ok((executor.take_output_state(), gas_used))
    }

    /// Fills in the post-execution header fields based on the given PostState and gas used.
    /// In doing this, the state root is calculated and the final header is returned.
    pub(crate) fn complete_header(
        &self,
        mut header: Header,
        bundle_state: &BundleStateWithReceipts,
        gas_used: u64,
        sk: &secp256k1::SecretKey,
        secp: &secp256k1::Secp256k1<secp256k1::All>,
        authorities: &Vec<secp256k1::PublicKey>,
        authority_to_vote_on: &Option<(secp256k1::PublicKey, Vote)>,
        recent_block_hash: bitcoin::BlockHash,
    ) -> Result<Header, BlockExecutionError> {
        let receipts = bundle_state.receipts_by_block(header.number);
        header.receipts_root = if receipts.is_empty() {
            EMPTY_RECEIPTS
        } else {
            let receipts_with_bloom = receipts
                .iter()
                .map(|r| (*r).clone().expect("receipts have not been pruned").into())
                .collect::<Vec<ReceiptWithBloom>>();
            header.logs_bloom =
                receipts_with_bloom.iter().fold(Bloom::ZERO, |bloom, r| bloom | r.bloom);
            proofs::calculate_receipt_root(&receipts_with_bloom)
        };
        header.gas_used = gas_used;

        let vote_for = if let Some(vote) = authority_to_vote_on { Some(vote.0) } else { None };
        // calculate the state root
        let state_root = self
            .client
            .latest()
            .map_err(|_| BlockExecutionError::ProviderError)?
            .state_root(bundle_state)
            .unwrap();
        header.state_root = state_root;

        // Serialize the header without signature
        let mut extra_header_content_no_signature = ExtraDataHeader::new(
            0u32,
            None,
            if header.is_poa_epoch() { Some(authorities.clone()) } else { None },
            vote_for,
            recent_block_hash,
        );
        let sig_hash = reth_consensus_common::utils::create_authority_sighash(
            &mut header.clone(),
            &extra_header_content_no_signature,
        );

        // Sign the header and append to extra data header
        let message =
            secp256k1::Message::from_slice(sig_hash.as_slice()).expect("Valid message to sign");
        let signature = secp.sign_ecdsa_recoverable(&message, sk);

        extra_header_content_no_signature.set_signature(signature.clone());

        header.extra_data = Bytes::from(extra_header_content_no_signature.serialize());
        Ok(header)
    }

    /// Builds and executes a new block with the given transactions, on the provided [Executor].
    ///
    /// This returns the header of the executed block, as well as the poststate from execution.
    pub(crate) fn build_and_execute<EvmConfig>(
        &mut self,
        transactions: Vec<TransactionSigned>,
        chain_spec: Arc<ChainSpec>,
        recent_block_header: Option<(bitcoin::block::Header, u32)>,
        vote: &Option<(secp256k1::PublicKey, Vote)>,
        sk: &secp256k1::SecretKey,
        secp: &secp256k1::Secp256k1<secp256k1::All>,
        authority_signers: &Vec<secp256k1::PublicKey>,
        evm_config: EvmConfig,
    ) -> Result<(SealedHeader, BundleStateWithReceipts), BlockExecutionError>
    where
        EvmConfig: ConfigureEvmEnv + Clone + 'static,
    {
        // Check if we have a recent block header
        // Can't validate pegin without it
        if recent_block_header.is_none() {
            return Err(BlockExecutionError::BitcoinRecentHeaderNotAvailable);
        }

        // Construct block and header
        let header =
            self.build_header_template(&transactions, &chain_spec.clone(), vote, sk, secp)?;

        let block = Block { header, body: transactions, ommers: vec![], withdrawals: None };
        let senders = TransactionSigned::recover_signers(&block.body, block.body.len())
            .ok_or(BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError))?;

        let block_with_senders = BlockWithSenders::new(block, senders).expect("senders are valid");

        trace!(target: "consensus::authority", transactions=?&block.body, "executing transactions");

        // Now execute the block
        let db = State::builder()
            .with_database_boxed(Box::new(StateProviderDatabase::new(
                self.client.latest().unwrap(),
            )))
            .with_bundle_update()
            .build();

        let mut executor = EVMProcessor::new_with_state(chain_spec.clone(), db, evm_config);

        let (bundle_state, gas_used) =
            self.execute(&block_with_senders, &mut executor, senders, recent_block_header)?;

        let Block { header, body, .. } = block;
        let body = BlockBody { transactions: body, ommers: vec![], withdrawals: None };

        trace!(target: "consensus::auto", ?bundle_state, ?header, ?body, "executed block, calculating state root and completing header");

        // fill in the rest of the fields
        let header = self.complete_header(
            header,
            &bundle_state,
            gas_used,
            sk,
            secp,
            &authority_signers,
            vote,
            // This is checked to be Some above
            recent_block_header.expect("valid header").0.block_hash(),
        )?;

        // Redundant check. Lets make sure the header is valid
        validate_poa_header_standalone(&header, &authority_signers).map_err(|e| {
            warn!(target: "consensus::authority", "failed to validate POA header: {:?}", e);
            // TODO(armins) return more expressive error
            BlockExecutionError::Validation(BlockValidationError::InvalidExtraData)
        })?;

        if vote.is_some() {
            let vote = vote.expect("valid vote");
            let authority_to_vote_on = vote.0;

            // TODO(armins) Should we be verbose and fail the block or just ignore?
            if authority_signers.iter().any(|signer| signer == &authority_to_vote_on) {
                return Err(BlockExecutionError::CannotAddExistingFederationMember);
            }
            // Keep track of votes
            self.authority_votes.vote_for(&sk.public_key(secp), &vote.1, &vote.0);
        }

        trace!(target: "consensus::authority", root=?header.state_root, ?body, "calculated root");
        let block_hash = header.hash_slow();
        let new_header = header.seal(block_hash);
        Ok((new_header, bundle_state))
    }

    // Execute and run poa validation on the block without inserting it into the storage
    pub(crate) fn execute_imported_block<EvmConfig>(
        &mut self,
        chain_spec: Arc<ChainSpec>,
        sealed_block: SealedBlock,
        recent_block_header: Option<(bitcoin::block::Header, u32)>,
        evm_config: EvmConfig,
    ) -> Result<BundleStateWithReceipts, BlockExecutionError>
    where
        EvmConfig: ConfigureEvmEnv + Clone + 'static,
    {
        // Check if we have a recent block header
        // Can't validate pegin without it
        if recent_block_header.is_none() {
            return Err(BlockExecutionError::BitcoinRecentHeaderNotAvailable);
        }
        trace!(target: "consensus::authority", transactions=?&sealed_block.body, "executing transactions");

        // Now execute the block
        let db = State::builder()
            .with_database_boxed(Box::new(StateProviderDatabase::new(
                self.client.latest().unwrap(),
            )))
            .with_bundle_update()
            .build();
        let mut executor = EVMProcessor::new_with_state(chain_spec.clone(), db, evm_config);

        let senders =
            TransactionSigned::recover_signers(&sealed_block.body, sealed_block.body.len()).ok_or(
                BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError),
            )?;

        let block_with_senders =
            BlockWithSenders::new(sealed_block.clone().unseal(), senders).expect("senders are valid");

        let (bundle_state, _gas_used) = self.execute(
            &block_with_senders,
            &mut executor,
            senders,
            recent_block_header,
        )?;

        let authority_signers = self.authorities.clone();
        validate_poa_header_standalone(&sealed_block.header.clone(), &authority_signers).map_err(
            |e| {
                warn!(target: "consensus::authority", "failed to validate POA header: {:?}", e);
                // TODO(armins) return more expressive error
                BlockExecutionError::Validation(BlockValidationError::InvalidExtraData)
            },
        )?;

        return Ok(bundle_state);
    }
}
