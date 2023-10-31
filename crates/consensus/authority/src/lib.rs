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
use botanix_lib::extra_data_header::{self, ExtraDataHeader};
use reth_consensus_common::validation;
use reth_interfaces::{
    consensus::{Consensus, ConsensusError},
    executor::{BlockExecutionError, BlockValidationError},
};
use reth_primitives::{
    constants::{
        EMPTY_RECEIPTS, EMPTY_TRANSACTIONS, ETHEREUM_BLOCK_GAS_LIMIT, MAXIMUM_EXTRA_DATA_SIZE,
    },
    proofs, Address, Block, BlockBody, BlockHash, BlockHashOrNumber, BlockNumber, Bloom, ChainSpec,
    Header, ReceiptWithBloom, SealedBlock, SealedHeader, TransactionSigned, EMPTY_OMMER_ROOT, H256,
    U256,
};
use reth_provider::{PostState, StateProvider};
use reth_revm::executor::Executor;
use reth_transaction_pool::TransactionPool;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use voting::{AuthorityVoteCollection, Vote};

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::{trace, warn};
mod builder;
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
        validation::validate_header_regarding_parent(parent, header, &self.chain_spec)?;
        Ok(())
    }

    fn validate_header_with_total_difficulty(
        &self,
        header: &Header,
        total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        if header.difficulty != U256::ZERO {
            return Err(ConsensusError::TheMergeDifficultyIsNotZero)
        }

        if header.ommers_hash != EMPTY_OMMER_ROOT {
            return Err(ConsensusError::TheMergeOmmerRootIsNotEmpty)
        }

        // validate header extradata
        validate_header_extradata(header)?;

        Ok(())
    }

    fn validate_block(&self, block: &SealedBlock) -> Result<(), ConsensusError> {
        validation::validate_block_standalone(block, &self.chain_spec)
    }
}

/// Validates the header's extradata according to the authority consensus rules.
///
/// From yellow paper: extraData: An arbitrary byte array containing data relevant to this block.
/// This must be 32 bytes or fewer; formally Hx.
fn validate_header_extradata(header: &Header) -> Result<(), ConsensusError> {
    if header.extra_data.len() > MAXIMUM_EXTRA_DATA_SIZE {
        Err(ConsensusError::ExtraDataExceedsMax { len: header.extra_data.len() })
    } else {
        // TODO (armins) check that no vote is occuring during an epoch header

        // 0. Validate that the block was signed by a federation member
        let extra_data =
            botanix_lib::extra_data_header::ExtraDataHeader::deserialize(header.extra_data)
                .map_err(|_e| {
                    ConsensusError::ExtraDataInvalid();
                })?;

        let sig_hash = utils::create_authority_sighash(header, &extra_data).map_err(|_e| {
            ConsensusError::ExtraDataInvalid();
        })?;

        header.validate_authority_signature(sig_hash).map_err(|_e| {
            ConsensusError::InvalidAuthoritySignature();
        })?;
        // 1. Validate that is a federation memeber was added or removed that that actions
        // was signed off by a 2/3 majority of votes
        // This can only happnen during an end of a epoch
        // TODO

        Ok(())
    }
}

/// In memory storage
#[derive(Debug, Clone, Default)]
pub(crate) struct Storage {
    inner: Arc<RwLock<StorageInner>>,
}

// == impl Storage ===
impl Storage {
    fn new(header: SealedHeader) -> Self {
        let (header, best_hash) = header.split();
        let mut storage = StorageInner {
            best_hash,
            total_difficulty: header.difficulty,
            best_block: header.number,
            ..Default::default()
        };
        storage.headers.insert(0, header);
        storage.bodies.insert(best_hash, BlockBody::default());
        Self { inner: Arc::new(RwLock::new(storage)) }
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

/// In-memory storage for the chain the authority seal engine is building.
#[derive(Default, Debug)]
pub(crate) struct StorageInner {
    /// Headers buffered for download.
    pub(crate) headers: HashMap<BlockNumber, Header>,
    /// A mapping between block hash and number.
    pub(crate) hash_to_number: HashMap<BlockHash, BlockNumber>,
    /// Bodies buffered for download.
    pub(crate) bodies: HashMap<BlockHash, BlockBody>,
    /// Tracks best block
    pub(crate) best_block: u64,
    /// Tracks hash of best block
    pub(crate) best_hash: H256,
    /// The total difficulty of the chain until this block
    pub(crate) total_difficulty: U256,
    /// Keep track of current votes
    pub(crate) authority_votes: AuthorityVoteCollection,
}

// === impl StorageInner ===

impl StorageInner {
    /// Returns the block hash for the given block number if it exists.
    pub(crate) fn block_hash(&self, num: u64) -> Option<BlockHash> {
        self.hash_to_number.iter().find_map(|(k, v)| num.eq(v).then_some(*k))
    }

    /// Returns the matching header if it exists.
    pub(crate) fn header_by_hash_or_number(
        &self,
        hash_or_num: BlockHashOrNumber,
    ) -> Option<Header> {
        let num = match hash_or_num {
            BlockHashOrNumber::Hash(hash) => self.hash_to_number.get(&hash).copied()?,
            BlockHashOrNumber::Number(num) => num,
        };
        self.headers.get(&num).cloned()
    }

    /// Inserts a new header+body pair
    pub(crate) fn insert_new_block(&mut self, mut header: Header, body: BlockBody) {
        header.number = self.best_block + 1;
        header.parent_hash = self.best_hash;

        self.best_hash = header.hash_slow();
        self.best_block = header.number;
        self.total_difficulty += header.difficulty;

        trace!(target: "consensus::authority", num=self.best_block, hash=?self.best_hash, "inserting new block");
        self.headers.insert(header.number, header);
        self.bodies.insert(self.best_hash, body);
        self.hash_to_number.insert(self.best_hash, self.best_block);
    }

    /// Fills in pre-execution header fields based on the current best block and given
    /// transactions.
    pub(crate) fn build_header_template(
        &self,
        transactions: &Vec<TransactionSigned>,
        chain_spec: Arc<ChainSpec>,
        vote: Option<(secp256k1::PublicKey, Vote)>,
        sk: &secp256k1::SecretKey,
        secp: &secp256k1::Secp256k1<secp256k1::All>,
    ) -> Result<Header, BlockExecutionError> {
        // check previous block for base fee
        let base_fee_per_gas = self
            .headers
            .get(&self.best_block)
            .and_then(|parent| parent.next_block_base_fee(chain_spec.base_fee_params));

        let mut header = Header {
            parent_hash: self.best_hash,
            ommers_hash: EMPTY_OMMER_ROOT,
            beneficiary: Default::default(),
            state_root: Default::default(),
            transactions_root: Default::default(),
            receipts_root: Default::default(),
            withdrawals_root: None,
            logs_bloom: Default::default(),
            difficulty: U256::from(2),
            number: self.best_block + 1,
            gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            gas_used: 0,
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            mix_hash: Default::default(),
            nonce: 0,
            base_fee_per_gas,
            blob_gas_used: None,
            excess_blob_gas: None,
            extra_data: Default::default(),
            parent_beacon_block_root: None,
        };

        let authority_to_vote_on = vote
            .is_some()
            .then(|| vote.expect("valid vote").0.serialize().expect("valid authority to vote on"));

        // Serialize the header without signature
        let extra_header_content_no_signature =
            ExtraDataHeader::new(0u32, None, chain_spec.authority_signer, authority_to_vote_on);
        header.extra_data = extra_header_content_no_signature.serialize_without_signature();

        let sig_hash = header.hash_slow();

        // Sign the header and append to extra data header
        let signature: secp256k1::schnorr::Signature = secp.sign_schnorr(sig_hash.as_slice(), sk);
        let extra_data_header_with_signature = ExtraDataHeader::new(
            0u32,
            Some(signature),
            chain_spec.authority_signers,
            authority_to_vote_on,
        );
        header.extra_data = extra_data_header_with_signature.serialize();

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
    pub(crate) fn execute<DB: StateProvider>(
        &mut self,
        block: &Block,
        executor: &mut Executor<DB>,
        senders: Vec<Address>,
        recent_block_header: Option<bitcoin::block::Header>,
    ) -> Result<(PostState, u64), BlockExecutionError> {
        trace!(target: "consensus::authority", transactions=?&block.body, "executing transactions");

        let (post_state, gas_used) =
            executor.execute_transactions(block, U256::ZERO, Some(senders), recent_block_header)?;

        // apply post block changes
        let post_state = executor.apply_post_block_changes(block, U256::ZERO, post_state)?;

        Ok((post_state, gas_used))
    }

    /// Fills in the post-execution header fields based on the given PostState and gas used.
    /// In doing this, the state root is calculated and the final header is returned.
    pub(crate) fn complete_header<DB: StateProvider>(
        &self,
        mut header: Header,
        post_state: &PostState,
        executor: &mut Executor<DB>,
        gas_used: u64,
    ) -> Header {
        let receipts = post_state.receipts(header.number);
        header.receipts_root = if receipts.is_empty() {
            EMPTY_RECEIPTS
        } else {
            let receipts_with_bloom =
                receipts.iter().map(|r| r.clone().into()).collect::<Vec<ReceiptWithBloom>>();
            header.logs_bloom =
                receipts_with_bloom.iter().fold(Bloom::zero(), |bloom, r| bloom | r.bloom);
            proofs::calculate_receipt_root(&receipts_with_bloom)
        };

        header.gas_used = gas_used;

        // calculate the state root
        let state_root = executor.db().db.0.state_root(post_state.clone()).unwrap();
        header.state_root = state_root;
        header
    }

    /// Builds and executes a new block with the given transactions, on the provided [Executor].
    ///
    /// This returns the header of the executed block, as well as the poststate from execution.
    pub(crate) fn build_and_execute<DB: StateProvider>(
        &mut self,
        transactions: Vec<TransactionSigned>,
        executor: &mut Executor<DB>,
        chain_spec: Arc<ChainSpec>,
        recent_block_header: Option<bitcoin::block::Header>,
        vote: Option<(secp256k1::PublicKey, Vote)>,
        sk: &secp256k1::SecretKey,
        secp: &secp256k1::Secp256k1<secp256k1::All>,
    ) -> Result<(SealedHeader, PostState), BlockExecutionError> {
        let header = self.build_header_template(&transactions, chain_spec, vote, sk, secp)?;

        let block = Block { header, body: transactions, ommers: vec![], withdrawals: None };
        let senders = TransactionSigned::recover_signers(&block.body, block.body.len())
            .ok_or(BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError))?;

        trace!(target: "consensus::authority", transactions=?&block.body, "executing transactions");

        // now execute the block
        let (post_state, gas_used) =
            self.execute(&block, executor, senders, recent_block_header)?;

        let Block { header, body, .. } = block;
        let body = BlockBody { transactions: body, ommers: vec![], withdrawals: None };

        trace!(target: "consensus::authority", ?post_state, ?header, ?body, "executed block, calculating state root and completing header");

        // fill in the rest of the fields
        let header = self.complete_header(header, &post_state, executor, gas_used);

        // TODO(armins) check if the authority being voted on has staked in the staking contract
        // TODO(armins) check if withdrawl is valid

        validate_header_extradata(&header).map_err(|e| {
            BlockExecutionError::Validation(BlockValidationError::InvalidExtraData())
        })?;

        if vote.is_some() {
            let vote = vote.expect("valid vote");
            let authority_to_vote_on = vote.expect("authority to vote on").0;
            let extra_data_header =
                extra_data_header::ExtraDataHeader::deserialize(header.extra_data.as_slice())
                    .map_err(|e| {
                        BlockExecutionError::Validation(
                            BlockValidationError::ExtraDataHeaderDeserialzeError(e),
                        )
                    })?;

            // TODO(armins) Should we be verbose and fail the block or just ignore?
            if let Some(_) = extra_data_header
                .authority_signers
                .iter()
                .any(|signer| signer == authority_to_vote_on)
            {
                return Err(BlockExecutionError::CannotAddExistingFederationMember)
            }
            // Keep track of votes
            self.authority_votes.vote_for(&sk.public_key(secp), vote.1, vote.0);
            trace!(target: "consensus::authority", vote, "casted vote");
        }

        trace!(target: "consensus::authority", root=?header.state_root, ?body, "calculated root");

        // finally insert into storage
        self.insert_new_block(header.clone(), body);

        // set new header with hash that should have been updated by insert_new_block
        let new_header = header.seal(self.best_hash);

        Ok((new_header, post_state))
    }
}
