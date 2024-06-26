pub(crate) mod AuthorityExecutionUtils {
    use bitcoin::hashes::sha256;
    use reth_consensus::{Consensus, ConsensusError};
    use reth_consensus_common::{
        utils::{
            get_block_producer_address, unix_timestamp, validate_extra_data_header_authorities,
        },
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
        proofs, public_key_to_address, Address, Block, BlockBody, BlockHashOrNumber,
        BlockWithSenders, Bloom, Bytes, ChainSpec, Header, ReceiptWithBloom, SealedBlock,
        SealedHeader, TransactionSigned, EMPTY_OMMER_ROOT_HASH, U256,
    };
    use reth_provider::{
        BlockExecutor, BlockReaderIdExt, BundleStateWithReceipts, ExecutorFactory,
        StateProviderFactory,
    };
    use reth_revm::{
        database::StateProviderDatabase, db::states::bundle_state::BundleRetention,
        processor::EVMProcessor, State,
    };
    use reth_rpc::eth::bundle;
    use std::sync::Arc;
    use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
    use tracing::{error, trace, warn};

    use crate::{AuthorityConsensus, AuthorityStorage};

    /// Fills in pre-execution header fields based on the current best block and given
    /// transactions.
    pub(crate) fn build_header_template(
        transactions: &[TransactionSigned],
        chain_spec: &Arc<ChainSpec>,
        client: &impl BlockReaderIdExt,
    ) -> Result<Header, BlockExecutionError> {
        // let (best_block, best_hash) = self.get_best_block_and_hash()?;
        let best_block =
            client.best_block_number().map_err(|e| BlockExecutionError::LatestBlock(e))?;
        let best_hash = client
            .block_hash(best_block)
            .map_err(|e| BlockExecutionError::LatestBlock(e))?
            .unwrap_or_else(|| {
                panic!("best block hash not found for block number: {}", best_block);
            });
        let timestamp = unix_timestamp();

        // check previous block for base fee
        let base_fee_per_gas = client
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

    /// Fills in the post-execution header fields based on the given PostState and gas used.
    /// In doing this, the state root is calculated and the final header is returned.
    pub(crate) fn complete_header(
        mut header: Header,
        bundle_state: &BundleStateWithReceipts,
        gas_used: u64,
        sk: &secp256k1::SecretKey,
        authorities: &[secp256k1::PublicKey],
        witness_data: &Option<Vec<bitcoin::witness::Witness>>,
        recent_block_hash: bitcoin::BlockHash,
        utxo_commitment: sha256::Hash,
        client: &(impl BlockReaderIdExt + StateProviderFactory),
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
        let state_root = client
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

    /// Builds and validates the current block header with the given transactions, on the provided
    /// [Executor].
    ///
    /// This returns the current block header.
    pub(crate) fn build_and_validate_header(
        bundle_state: &BundleStateWithReceipts,
        block: Block,
        gas_used: u64,
        botanix_consensus_pkg: Option<BotanixConsensusPackage>,
        sk: &secp256k1::SecretKey,
        authority_signers: &Vec<secp256k1::PublicKey>,
        witness_data: &Option<Vec<bitcoin::witness::Witness>>,
        utxo_commitment: sha256::Hash,
        consensus: &AuthorityConsensus,
        client: &(impl BlockReaderIdExt + StateProviderFactory),
        authority_storage: &impl AuthorityStorage,
    ) -> Result<SealedHeader, BlockExecutionError> {
        let Block { header, body, .. } = block;
        let body = BlockBody { transactions: body, ommers: vec![], withdrawals: None };

        // fill in the rest of the fields
        let header = complete_header(
            header,
            bundle_state,
            gas_used,
            sk,
            authority_signers,
            witness_data,
            // This is checked to be Some above
            botanix_consensus_pkg.expect("consensus pkg").bitcoin_checkpoint.0.block_hash(),
            utxo_commitment,
            client,
        )?;

        // Validate EDH authorities match genesis authorities
        if let Err(e) = validate_extra_data_header_authorities(
            &header,
            &authority_storage.get_genesis_authorities(),
        ) {
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
    pub(crate) fn execute_imported_block(
        consensus: &AuthorityConsensus,
        sealed_block: SealedBlock,
        botanix_consensus_pkg: Option<BotanixConsensusPackage>,
        client: &(impl BlockReaderIdExt + StateProviderFactory),
        executor_factory: &impl ExecutorFactory,
        authority_storage: &impl AuthorityStorage,
    ) -> Result<BundleStateWithReceipts, BlockExecutionError> {
        trace!(target: "consensus::authority", transactions=?&sealed_block.body, "executing transactions");
        let senders =
            TransactionSigned::recover_signers(&sealed_block.body, sealed_block.body.len()).ok_or(
                BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError),
            )?;

        let block_with_senders =
            BlockWithSenders::new(sealed_block.clone().unseal(), senders.clone())
                .expect("senders are valid");

        // validate before executing block
        let authority_signers = authority_storage.get_authorities();
        let genesis_authorities = authority_storage.get_genesis_authorities();
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
        let (bundle_state, _gas_used) = execute(
            &block_with_senders,
            executor_factory,
            client,
            botanix_consensus_pkg,
            Some(block_builder_address),
        )?;

        Ok(bundle_state)
    }

    /// Executes the block with the given block and senders, on the provided [Executor].
    ///
    /// This returns the poststate from execution and post-block changes, as well as the gas used.
    pub(crate) fn execute(
        block: &BlockWithSenders,
        executor_factory: &impl ExecutorFactory,
        client: &(impl StateProviderFactory + BlockReaderIdExt),
        botanix_consensus_pkg: Option<BotanixConsensusPackage>,
        block_builder_address: Option<Address>,
    ) -> Result<(BundleStateWithReceipts, u64), BlockExecutionError> {
        let db = client.latest().map_err(|e| BlockExecutionError::LatestBlock(e))?;
        let mut executor = executor_factory.with_state(db);

        let (receipts, gas_used, total_block_fees) =
            executor.execute_transactions(block, U256::ZERO, botanix_consensus_pkg)?;
        let bundle_state = executor.take_output_state();

        // set the first block to find the correct index in bundle state
        bundle_state.set_first_block(block.number);

        // Save receipts.
        bundle_state.save_receipts(receipts)?;

        // add post execution state change
        // Withdrawals, rewards etc.
        bundle_state.apply_post_execution_state_change(
            block,
            U256::ZERO,
            Some(total_block_fees),
            block_builder_address,
        )?;

        // merge transitions
        bundle_state.db_mut().merge_transitions(BundleRetention::Reverts);

        // apply post block changes
        Ok((bundle_state, gas_used))
    }
}
