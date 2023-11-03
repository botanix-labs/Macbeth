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
#![feature(noop_waker)]

//! A [Consensus] implementation of Clique Proof of Authority (POA)
//! that authoritymatically seals blocks.
//!
//! The Mining task polls a [MiningMode], and will return a list of transactions that are ready to
//! be mined.
//!
//! These downloaders poll the miner, assemble the block, and return transactions that are ready to
//! be mined.
use reth_botanix_lib::extra_data_header::{self, ExtraDataHeader};
use reth_consensus_common::{utils, validation};
use reth_interfaces::{
    consensus::{Consensus, ConsensusError},
    executor::{BlockExecutionError, BlockValidationError},
};
use reth_primitives::{
    constants::{
        eip225::SIGNER_LIMIT, EMPTY_RECEIPTS, EMPTY_TRANSACTIONS, ETHEREUM_BLOCK_GAS_LIMIT,
    },
    proofs, Address, Block, BlockBody, BlockHash, BlockHashOrNumber, BlockNumber, Bloom, Bytes,
    ChainSpec, Header, ReceiptWithBloom, SealedBlock, SealedHeader, TransactionSigned,
    EMPTY_OMMER_ROOT_HASH, H256, U256,
};
use reth_provider::{
    BlockExecutor, BlockReaderIdExt, BundleStateWithReceipts, CanonStateNotificationSender,
    StateProviderFactory,
};
use reth_revm::{
    database::StateProviderDatabase, db::states::bundle_state::BundleRetention,
    processor::EVMProcessor, State,
};
use std::{
    collections::HashMap,
    f32::consts::E,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use voting::{AuthorityVoteCollection, Vote};

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::{info, trace, warn};
mod builder;
mod client;
mod epoch_manager;
mod task;
mod voting;
mod constants;
mod epoch_manager;
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
        // TODO (armins) get prev headers from storage and validate signer limit
        // self.validate_signer_limit(header, prev_headers)?;
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

// POA specific functions
impl AuthorityConsensus {
    /// Validates that the number of signers is within the allowed limit.
    ///
    /// # Returns
    /// `Ok(())` if the number of signers is within the allowed limit, otherwise
    /// returns an error indicating the validation failed.
    fn validate_signer_limit(
        &self,
        header: &SealedHeader,
        prev_headers: Vec<Header>,
    ) -> Result<(), ConsensusError> {
        if prev_headers.len() < SIGNER_LIMIT as usize {
            return Ok(())
        }

        let signer = utils::recovery_authority(header)
            .map_err(|_| ConsensusError::FailedToRecoverAuthority)?;
        if prev_headers.into_iter().any(|prev_header| {
            let prev_signer = utils::recovery_authority(&prev_header);
            prev_signer.is_err() || signer == prev_signer.expect("valid signer")
        }) {
            return Err(ConsensusError::SignerLimitReached)
        }
        Ok(())
    }
}

// POA specific functions
impl AuthorityConsensus {
    /// Validates that the number of signers is within the allowed limit.
    ///
    /// # Returns
    /// `Ok(())` if the number of signers is within the allowed limit, otherwise
    /// returns an error indicating the validation failed.
    fn validate_signer_limit(
        &self,
        header: &SealedHeader,
        prev_headers: Vec<Header>,
    ) -> Result<(), ConsensusError> {
        if prev_headers.len() < SIGNER_LIMIT as usize {
            return Ok(())
        }

        let signer = utils::recovery_authority(header)
            .map_err(|_| ConsensusError::FailedToRecoverAuthority)?;
        if prev_headers.into_iter().any(|prev_header| {
            let prev_signer = utils::recovery_authority(&prev_header)
                .map_err(|_| ConsensusError::FailedToRecoverAuthority)?;

            signer == prev_signer
        }) {
            return Err(ConsensusError::SignerLimitReached)
        }
        Ok(())
    }
}

/// Validates the header's extradata according to the authority consensus rules.
///
/// From yellow paper: extraData: An arbitrary byte array containing data relevant to this block.
/// This must be 32 bytes or fewer; formally Hx.
fn validate_header_extradata(header: &Header) -> Result<(), ConsensusError> {
    // Skip over genesis
    if header.number == 0 {
        return Ok(())
    }
    // TODO (armins) calculate worst case max size
    // if header.extra_data.len() > MAXIMUM_EXTRA_DATA_SIZE {
    //     Err(ConsensusError::ExtraDataExceedsMax { len: header.extra_data.len() })
    // } else

    {
        // TODO (armins) check that no vote is occuring during an epoch header
        // 0. Validate that the block was signed by a federation member
        let extra_data = reth_botanix_lib::extra_data_header::ExtraDataHeader::deserialize(
            &mut header.extra_data.0.to_vec().as_slice(),
        )
        .map_err(|_e| ConsensusError::ExtraDataInvalid)?;

        // 0. Validate that the block was signed by a federation member
        let extra_data = reth_botanix_lib::extra_data_header::ExtraDataHeader::deserialize(
            &mut header.extra_data.0.to_vec().as_slice(),
        )
        .map_err(|e| {
            info!("Failed to deserialize extra data header {:?}", e);
            ConsensusError::ExtraDataInvalid
        })?;
        let sig_hash = utils::create_authority_sighash(&mut header.clone(), &extra_data);
        extra_data.validate_authority_signature(&sig_hash.to_vec()).map_err(|e| {
            info!("Failed to validate authority signature, {:?} ", e);
            ConsensusError::InvalidAuthoritySignature
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
    pub(crate) inner: Arc<RwLock<StorageInner>>,
}

#[derive(Debug)]
pub(crate) enum StorageCreationError {
    /// empty headers
    EmptyHeaders,
}

// == impl Storage ===
impl Storage {
    fn try_new(
        headers: &mut Vec<SealedHeader>,
        authorities: Vec<secp256k1::PublicKey>,
        signer_index: usize,
    ) -> Result<Self, StorageCreationError> {
        if headers.len() == 0 {
            return Err(StorageCreationError::EmptyHeaders)
        }
        // sort the headers by block numbers
        headers.sort_by(|a, b| a.number.cmp(&b.number));

        // We need to start storing headers from the start of the epoch
        let (header, best_hash) = headers.get(0).expect("valid index").clone().split();

        let mut storage = StorageInner {
            best_hash,
            total_difficulty: header.difficulty,
            best_block: header.number,
            authorities,
            signer_index,
            ..Default::default()
        };
        storage.headers.insert(header.number, header);
        storage.bodies.insert(best_hash, BlockBody::default());

        Ok(Self { inner: Arc::new(RwLock::new(storage)) })
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
    /// Keep track of the  signers
    pub(crate) authorities: Vec<secp256k1::PublicKey>,
    /// keep track of my place among the singer
    /// This will change as new signers are removed
    pub(crate) signer_index: usize,
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
        vote: &Option<(secp256k1::PublicKey, Vote)>,
    ) -> Result<Header, BlockExecutionError> {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        // check previous block for base fee
        let base_fee_per_gas = self
            .headers
            .get(&self.best_block)
            .and_then(|parent| parent.next_block_base_fee(chain_spec.base_fee_params(timestamp)));

        let mut header = Header {
            parent_hash: self.best_hash,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
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
    pub(crate) fn execute(
        &mut self,
        block: &Block,
        executor: &mut EVMProcessor<'_>,
        senders: Vec<Address>,
        recent_block_header: Option<(bitcoin::blockdata::block::Header, u32)>,
    ) -> Result<(BundleStateWithReceipts, u64), BlockExecutionError> {
        // set the first block to find the correct index in bundle state
        executor.set_first_block(block.number);

        let (receipts, gas_used) =
            executor.execute_transactions(block, U256::ZERO, Some(senders), recent_block_header)?;

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
    pub(crate) fn complete_header<S: StateProviderFactory>(
        &self,
        mut header: Header,
        bundle_state: &BundleStateWithReceipts,
        client: &S,
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
        // calculate the state root
        let state_root = client
            .latest()
            .map_err(|_| BlockExecutionError::ProviderError)?
            .state_root(bundle_state)
            .unwrap();
        header.state_root = state_root;

        // next up is POA specific stuff

        // Sign the header
        // Serialize the header without signature
        // TODO signing list should come from prev blocl header
        let extra_header_content_no_signature =
            ExtraDataHeader::new(0u32, None, authorities.clone(), vote_for, recent_block_hash);

        let sig_hash = utils::create_authority_sighash(
            &mut header.clone(),
            &extra_header_content_no_signature,
        );
        // Sign the header and append to extra data header
        let message =
            secp256k1::Message::from_slice(sig_hash.as_slice()).expect("Valid message to sign");
        let signature = secp.sign_ecdsa_recoverable(&message, sk);
        let extra_data_header_with_signature = ExtraDataHeader::new(
            0u32,
            Some(signature),
            authorities.clone(),
            vote_for,
            recent_block_hash,
        );
        header.extra_data = Bytes::from(extra_data_header_with_signature.serialize());

        Ok(header)
    }

    /// Builds and executes a new block with the given transactions, on the provided [Executor].
    ///
    /// This returns the header of the executed block, as well as the poststate from execution.
    pub(crate) fn build_and_execute(
        &mut self,
        transactions: Vec<TransactionSigned>,
        client: &impl StateProviderFactory,
        chain_spec: Arc<ChainSpec>,
        recent_block_header: Option<(bitcoin::block::Header, u32)>,
        vote: &Option<(secp256k1::PublicKey, Vote)>,
        sk: &secp256k1::SecretKey,
        secp: &secp256k1::Secp256k1<secp256k1::All>,
    ) -> Result<(SealedHeader, BundleStateWithReceipts), BlockExecutionError> {
        if recent_block_header.is_none() {
            return Err(BlockExecutionError::BitcoinRecentHeaderNotAvailable)
        }

        // Retrieve previous block header
        let prev_header = self.headers.get(&self.best_block).expect("checked empty");
        let signers = utils::get_authority_list(prev_header).map_err(|e| {
            println!("deserialize error {:?}", e);
            BlockExecutionError::FailedToDeserializePreviousBlockHeader
        })?;

        let header = self.build_header_template(&transactions, chain_spec.clone(), vote)?;

        let block = Block { header, body: transactions, ommers: vec![], withdrawals: None };
        let senders = TransactionSigned::recover_signers(&block.body, block.body.len())
            .ok_or(BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError))?;

        trace!(target: "consensus::authority", transactions=?&block.body, "executing transactions");

        // now execute the block
        let db = State::builder()
            .with_database_boxed(Box::new(StateProviderDatabase::new(client.latest().unwrap())))
            .with_bundle_update()
            .build();
        let mut executor = EVMProcessor::new_with_state(chain_spec.clone(), db);

        let (bundle_state, gas_used) =
            self.execute(&block, &mut executor, senders, recent_block_header)?;

        let Block { header, body, .. } = block;
        let body = BlockBody { transactions: body, ommers: vec![], withdrawals: None };

        trace!(target: "consensus::auto", ?bundle_state, ?header, ?body, "executed block, calculating state root and completing header");

        // fill in the rest of the fields
        let header = self.complete_header(
            header,
            &bundle_state,
            client,
            gas_used,
            sk,
            secp,
            &signers,
            vote,
            // This is checked to be Some above
            recent_block_header.expect("valid header").0.block_hash(),
        )?;

        // TODO(armins) check if the authority being voted on has staked in the staking contract
        // TODO(armins) check if withdrawl is valid

        validation::validate_header_extradata(&header).map_err(|_e| {
            BlockExecutionError::Validation(BlockValidationError::InvalidExtraData)
        })?;

        if vote.is_some() {
            let vote = vote.expect("valid vote");
            let authority_to_vote_on = vote.0;

            // TODO(armins) Should we be verbose and fail the block or just ignore?
            if signers.iter().any(|signer| signer == &authority_to_vote_on) {
                return Err(BlockExecutionError::CannotAddExistingFederationMember)
            }
            // Keep track of votes
            self.authority_votes.vote_for(&sk.public_key(secp), &vote.1, &vote.0);
        }

        // TODO(armins) check if the authority being voted on has staked in the staking contract
        // TODO(armins) check if withdrawl is valid

        validate_header_extradata(&header).map_err(|_e| {
            BlockExecutionError::Validation(BlockValidationError::InvalidExtraData)
        })?;

        // TODO(armins) Validate signer limit

        if vote.is_some() {
            let vote = vote.expect("valid vote");
            let authority_to_vote_on = vote.0;
            let extra_data_header = extra_data_header::ExtraDataHeader::deserialize(
                &mut header.extra_data.0.to_vec().as_slice(),
            )
            .map_err(|e| {
                BlockExecutionError::Validation(BlockValidationError::ExtraDataSerializeError)
            })?;

            // TODO(armins) Should we be verbose and fail the block or just ignore?
            if extra_data_header
                .authority_signers
                .iter()
                .any(|signer| signer == &authority_to_vote_on)
            {
                return Err(BlockExecutionError::CannotAddExistingFederationMember)
            }
            // Keep track of votes
            self.authority_votes.vote_for(&sk.public_key(secp), &vote.1, &vote.0);
        }

        trace!(target: "consensus::authority", root=?header.state_root, ?body, "calculated root");

        // finally insert into storage
        self.insert_new_block(header.clone(), body);

        // set new header with hash that should have been updated by insert_new_block
        let new_header = header.seal(self.best_hash);

        Ok((new_header, bundle_state))
    }
}
