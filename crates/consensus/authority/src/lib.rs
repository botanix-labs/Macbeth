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

use bitcoin::hashes::sha256;
use reth_consensus::{Consensus, ConsensusError};
use reth_consensus_common::{
    utils::{get_block_producer_address, unix_timestamp, validate_extra_data_header_authorities},
    validation::{self},
};
use reth_interfaces::{
    executor::{BlockExecutionError, BlockValidationError},
    provider::ProviderError,
};
use reth_node_api::ConfigureEvmEnv;
use reth_primitives::{
    botanix::BotanixConsensusPackage,
    constants::{EMPTY_RECEIPTS, EMPTY_TRANSACTIONS, ETHEREUM_BLOCK_GAS_LIMIT},
    extra_data_header::ExtraDataHeader,
    header_ext::HeaderExt,
    proofs, public_key_to_address,
    revm_primitives::FixedBytes,
    Address, Block, BlockBody, BlockHash, BlockHashOrNumber, BlockWithSenders, Bloom, Bytes,
    ChainSpec, Header, ReceiptWithBloom, SealedBlock, SealedHeader, TransactionSigned, B256,
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
use std::sync::Arc;
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::{error, trace, warn};

mod block_builder;
mod block_fetcher;
mod builder;
mod dkg;
mod engine_util;
mod epoch_manager;
pub mod extended_client;
mod frost_task;
mod healthcheck_task;
pub mod notifications;
mod pbft;
mod pbft_task;
mod signing;
mod sync;
mod task;
pub mod utils;
pub use builder::AuthorityConsensusBuilder;

/// Block time duration (secs)
pub const BLOCK_TIME_DURATION_SECS: u64 = 1 * 60;

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
        // TODO(armins) most likely we need to pass storage here
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
        reth_consensus_common::utils::validate_against_parent(
            parent.header().clone(),
            header.header().clone(),
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

        if header.is_poa_epoch() {
            // Validate the list of authorities matches the authorities in the genesis block
            // This check is only for a static federation
            // Use EDH authority list as source of truth and not the list passed in
            if genesis_authorities !=
                edh.authority_signers.as_ref().expect("authority signers to exist")
            {
                error!("Genesis authorities: {:?}", genesis_authorities);
                error!("EDH authorities: {:?}", edh.authority_signers);
                return Err(ConsensusError::InvalidAuthorityList);
            }
        }

        // Validate a quorum of authority signatures
        let valid_sigs = header.check_authority_sig_add(authority_signers).map_err(|e| {
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
    ) -> Result<(), ConsensusError> {
        // run the reth header validation rule
        let sealed_header = header.clone().seal_slow();
        reth_consensus_common::validation::validate_header_standalone(
            &sealed_header,
            &self.chain_spec,
        )?;

        // Validate EDH serialization and signature on block
        self.validate_extra_data_header(header, authority_signers, genesis_authorities)?;

        // Validate fee benificiary
        self.validate_block_beneficiary(header)?;

        // Validate signer is in turn
        header
            .validate_inturn(authority_signers)
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

    /// Validate poa extra data header
    /// This function will validate the extra data header and check for a quorum of signatures
    /// from authorities memebers.
    /// TODO (armins) validate only 2/3 of the authorities have signed, rn we are checking for n
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
        let _edh = header.deserialize_extra_data_header().map_err(|e| {
            error!("Failed to deserialize extra data header: {:?}", e);
            ConsensusError::ExtraDataInvalid
        })?;
        // Validate the authority signature and signature came from one of the authorities
        header.validate_first_authority_signature(authority_signers).map_err(|e| {
            error!("Failed to validate authority signature: {:?}", e);
            ConsensusError::InvalidAuthoritySignature
        })?;

        // Validate fee benificiary
        self.validate_block_beneficiary(header)?;

        // Validate signer is in turn
        header
            .validate_inturn(authority_signers)
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
        headers: &mut [SealedHeader],
        genesis_authorities: Vec<secp256k1::PublicKey>,
        authorities: Vec<secp256k1::PublicKey>,
        signer_index: usize,
        pk: secp256k1::PublicKey,
        btc_network: bitcoin::Network,
    ) -> Result<Self, StorageCreationError> {
        if headers.is_empty() {
            return Err(StorageCreationError::EmptyHeaders);
        }
        // sort the headers by block numbers
        headers.sort_by(|a, b| a.number.cmp(&b.number));

        let storage = StorageInner {
            client: client.clone(),
            genesis_authorities,
            authorities,
            signer_index,
            authority: pk,
            aggregate_public_key: None,
            btc_network,
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
    pub(crate) btc_network: bitcoin::Network,
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
        let best_block = self
            .client
            .best_block_number()
            .map_err(|_| BlockExecutionError::LatestBlock(ProviderError::BestBlockNotFound))?;

        let best_hash = self
            .client
            .block_hash(best_block)
            .map_err(|_| {
                // can't pass block number only hash
                BlockExecutionError::LatestBlock(ProviderError::BlockHashNotFound(B256::ZERO))
            })?
            .unwrap_or_else(|| {
                panic!("{}", format!("Missing block hash for best block {:?}", best_block))
            });

        Ok((best_block, best_hash))
    }

    /// Fills in pre-execution header fields based on the current best block and given
    /// transactions.
    pub(crate) fn build_header_template(
        &self,
        transactions: &[TransactionSigned],
        chain_spec: &Arc<ChainSpec>,
    ) -> Result<Header, BlockExecutionError> {
        let (best_block, best_hash) = self.get_best_block_and_hash()?;
        let timestamp = unix_timestamp();

        // check previous block for base fee
        let base_fee_per_gas = self
            .client
            .header_by_hash_or_number(BlockHashOrNumber::Number(best_block))
            .expect("header to exist")
            .and_then(|parent| {
                parent.next_block_base_fee(chain_spec.base_fee_params_at_timestamp(timestamp))
            });

        let mut header = Header {
            parent_hash: best_hash,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: Address::ZERO, // burn the block reward so not to increase ether supply
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
        _senders: Vec<Address>,
        botanix_consensus_pkg: Option<BotanixConsensusPackage>,
        block_builder_address: Option<Address>,
    ) -> Result<(BundleStateWithReceipts, u64), BlockExecutionError>
    where
        EvmConfig: ConfigureEvmEnv + Clone + 'static + reth_node_api::ConfigureEvm,
    {
        // set the first block to find the correct index in bundle state
        executor.set_first_block(block.number);

        let (receipts, gas_used, total_block_fees) =
            executor.execute_transactions(block, U256::ZERO, botanix_consensus_pkg)?;

        // Save receipts.
        executor.save_receipts(receipts)?;

        // add post execution state change
        // Withdrawals, rewards etc.
        executor.apply_post_execution_state_change(
            block,
            U256::ZERO,
            Some(total_block_fees),
            block_builder_address,
        )?;

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
        authorities: &[secp256k1::PublicKey],
        witness_data: &Option<Vec<bitcoin::witness::Witness>>,
        recent_block_hash: bitcoin::BlockHash,
        utxo_commitment: sha256::Hash,
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

        // calculate the state root
        let state_root = self
            .client
            .latest()
            .map_err(|_| {
                BlockExecutionError::LatestBlock(ProviderError::StateForHashNotFound(
                    header.hash_slow(),
                ))
            })?
            .state_root(bundle_state.state())
            .unwrap();
        header.state_root = state_root;

        // fail if witness data is empty
        // witness data will be None if no pegouts are being processed in this block
        if let Some(witness_data) = witness_data {
            if witness_data.is_empty() {
                return Err(BlockExecutionError::Validation(
                    BlockValidationError::MissingWitnessData,
                ));
            }
        };

        // Construct [ExtraDataHeader] and sign the block
        let edh = ExtraDataHeader::new(
            0,
            None,
            if header.is_poa_epoch() { Some(authorities.to_vec()) } else { None },
            None,
            witness_data.clone(),
            recent_block_hash,
            utxo_commitment,
        );
        header.extra_data = Bytes::from(edh.serialize());
        header.sign_block(&sk).map_err(|e| {
            warn!(target: "consensus::authority", "failed to sign block: {:?}", e);
            BlockExecutionError::Validation(BlockValidationError::InvalidExtraData)
        })?;
        Ok(header)
    }

    //// Builds and executes a new block with the given transactions, on the provided [Executor].
    ///
    /// This returns bundle state, block, and gas used.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_and_execute<EvmConfig>(
        &mut self,
        transactions: Vec<TransactionSigned>,
        chain_spec: Arc<ChainSpec>,
        botanix_consensus_pkg: Option<BotanixConsensusPackage>,
        sk: &secp256k1::SecretKey,
        evm_config: EvmConfig,
    ) -> Result<(BundleStateWithReceipts, Block, u64), BlockExecutionError>
    where
        EvmConfig: ConfigureEvmEnv + Clone + 'static + reth_node_api::ConfigureEvm,
    {
        // Check if we have a recent block header
        // Can't validate pegin without it
        if botanix_consensus_pkg.is_none() {
            return Err(BlockExecutionError::BitcoinRecentHeaderNotAvailable);
        }

        // Construct block and header
        let header = self.build_header_template(&transactions, &chain_spec.clone())?;

        let block = Block { header, body: transactions, ommers: vec![], withdrawals: None };
        let senders = TransactionSigned::recover_signers(&block.body, block.body.len())
            .ok_or(BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError))?;

        let block_with_senders =
            BlockWithSenders::new(block.clone(), senders.clone()).expect("senders are valid");

        trace!(target: "consensus::authority", transactions=?&block.body, "executing transactions");

        // Now execute the block
        let db = State::builder()
            .with_database_boxed(Box::new(StateProviderDatabase::new(
                self.client.latest().unwrap(),
            )))
            .with_bundle_update()
            .build();

        let mut executor = EVMProcessor::new_with_state(chain_spec.clone(), db, evm_config);

        // derive block builder address to receive block fees
        let block_builder_pub_key = secp256k1::PublicKey::from_secret_key_global(sk);
        let block_builder_address = public_key_to_address(block_builder_pub_key);
        let (bundle_state, gas_used) = self.execute(
            &block_with_senders,
            &mut executor,
            senders,
            botanix_consensus_pkg.clone(),
            Some(block_builder_address),
        )?;
        Ok((bundle_state, block, gas_used))
    }

    /// Builds and validates the current block header with the given transactions, on the provided
    /// [Executor].
    ///
    /// This returns the current block header.
    pub(crate) fn build_and_validate_header(
        &mut self,
        bundle_state: &BundleStateWithReceipts,
        block: Block,
        gas_used: u64,
        botanix_consensus_pkg: Option<BotanixConsensusPackage>,
        sk: &secp256k1::SecretKey,
        authority_signers: &Vec<secp256k1::PublicKey>,
        witness_data: &Option<Vec<bitcoin::witness::Witness>>,
        utxo_commitment: sha256::Hash,
        consensus: &AuthorityConsensus,
    ) -> Result<SealedHeader, BlockExecutionError> {
        let Block { header, body, .. } = block;
        let body = BlockBody { transactions: body, ommers: vec![], withdrawals: None };

        // fill in the rest of the fields
        let header = self.complete_header(
            header,
            bundle_state,
            gas_used,
            sk,
            authority_signers,
            witness_data,
            // This is checked to be Some above
            botanix_consensus_pkg.expect("consensus pkg").bitcoin_checkpoint.0.block_hash(),
            utxo_commitment,
        )?;

        // Validate EDH authorities match genesis authorities
        if let Err(e) = validate_extra_data_header_authorities(&header, &self.genesis_authorities) {
            error!(target: "consensus::authority", "failed to validate EDH authorities: {:?}", e);
            return Err(BlockExecutionError::Validation(
                BlockValidationError::InvalidExtraDataAuthorities,
            ));
        }

        // Redundant check. Lets make sure the header is valid
        consensus.validate_extra_data_header_single_signer(&header, authority_signers).map_err(
            |e| {
                warn!(target: "consensus::authority", "failed to validate POA header: {:?}", e);
                // TODO(armins) return more expressive error
                BlockExecutionError::Validation(BlockValidationError::InvalidExtraData)
            },
        )?;

        trace!(target: "consensus::authority", root=?header.state_root, ?body, "calculated root");
        let block_hash = header.hash_slow();
        let new_header = header.seal(block_hash);
        Ok(new_header)
    }

    // Execute and run poa validation on the block without inserting it into the storage
    pub(crate) fn execute_imported_block<EvmConfig>(
        &mut self,
        consensus: &AuthorityConsensus,
        sealed_block: SealedBlock,
        botanix_consensus_pkg: Option<BotanixConsensusPackage>,
        evm_config: EvmConfig,
    ) -> Result<BundleStateWithReceipts, BlockExecutionError>
    where
        EvmConfig: ConfigureEvmEnv + Clone + 'static + reth_node_api::ConfigureEvm,
    {
        trace!(target: "consensus::authority", transactions=?&sealed_block.body, "executing transactions");

        // Now execute the block
        let db = State::builder()
            .with_database_boxed(Box::new(StateProviderDatabase::new(
                self.client.latest().unwrap(),
            )))
            .with_bundle_update()
            .build();
        let mut executor =
            EVMProcessor::new_with_state(consensus.chain_spec.clone(), db, evm_config);

        let senders =
            TransactionSigned::recover_signers(&sealed_block.body, sealed_block.body.len()).ok_or(
                BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError),
            )?;

        let block_with_senders =
            BlockWithSenders::new(sealed_block.clone().unseal(), senders.clone())
                .expect("senders are valid");

        // validate before executing block
        let authority_signers = self.authorities.clone();
        let genesis_authorities = self.genesis_authorities.clone();
        consensus
            .validate_header_standalone(
                &sealed_block.header.clone(),
                &authority_signers,
                &genesis_authorities,
            )
            .map_err(|e| {
                warn!(target: "consensus::authority", "failed to validate POA header: {:?}", e);
                // TODO(armins) return more expressive error
                BlockExecutionError::Validation(BlockValidationError::InvalidExtraData)
            })?;

        let block_builder_address = get_block_producer_address(&sealed_block.header.clone());
        let (bundle_state, _gas_used) = self.execute(
            &block_with_senders,
            &mut executor,
            senders,
            botanix_consensus_pkg,
            Some(block_builder_address),
        )?;

        Ok(bundle_state)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use reth_consensus_common::utils::{
        block_fees_split, current_inturn_index, get_in_turn_interval, is_inturn,
        validate_against_parent,
    };
    use reth_primitives::BOTANIX_TESTNET;

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
        let result =
            consensus.validate_extra_data_header(&header, &authority_signers, &authority_signers);

        assert!(result.is_ok());
    }

    #[test]
    fn should_fail_on_invalid_signature() {
        let consensus = AuthorityConsensus::new(Arc::new(BOTANIX_TESTNET.as_ref().to_owned()));
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

        let result =
            consensus.validate_extra_data_header(&header, &authority_signers, &authority_signers);
        assert!(result.is_err());

        // reset header and try again with a
        let mut header = Header::default();
        header.number = 1;
        header.extra_data = Bytes::from(edh.serialize());
        header.sign_block(&sk1).expect("valid sign");

        let result =
            consensus.validate_extra_data_header(&header, &authority_signers, &authority_signers);
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

    #[test]
    fn get_inturn_interval_secs_based() {
        let current_ts = super::unix_timestamp();
        let authorities_len = 10;
        let current_in_turn_signer = current_inturn_index(authorities_len, current_ts);
        let (start, end, time_passed, time_remaining) =
            get_in_turn_interval(authorities_len, current_in_turn_signer, current_ts);

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
