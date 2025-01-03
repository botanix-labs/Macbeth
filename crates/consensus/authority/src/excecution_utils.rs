pub(crate) mod authority_execution_utils {
    use reth_btc_wallet::bitcoind::BitcoindFactory;
    use reth_chainspec::{ChainSpec, EthereumHardforks};

    use reth_db::Database;
    use reth_evm::execute::{BatchExecutor, BlockExecutorProvider, Executor};
    use reth_evm_ethereum::execute::EthBlockExecutor;
    use reth_execution_errors::{
        BlockExecutionError, BlockValidationError, InternalBlockExecutionError,
    };
    use reth_node_ethereum::EthEvmConfig;
    use reth_primitives::{
        botanix::block_with_peg::SealedBlockWithPeg,
        constants::{EMPTY_RECEIPTS, EMPTY_TRANSACTIONS, ETHEREUM_BLOCK_GAS_LIMIT},
        eip4844::calculate_excess_blob_gas,
        extra_data_header::{ExtraDataHeader, CHAIN_VERSION, EXTRA_HEADER_VERSION_1},
        header_ext::HeaderExt,
        proofs, Address, Block, BlockHashOrNumber, BlockWithSenders, Bloom, Bytes, Header, Receipt,
        ReceiptWithBloom, Requests, TransactionSigned, EMPTY_OMMER_ROOT_HASH, U256,
    };
    use reth_provider::{
        BlockExecutionInput, BlockExecutionOutput, BlockHashReader, BlockNumReader,
        ExecutionOutcome, HeaderProvider, ProviderFactory,
    };
    use reth_revm::{database::StateProviderDatabase, db::State};
    use reth_trie::StateRoot;
    use reth_trie_db::DatabaseStateRoot;

    use std::sync::Arc;
    use tendermint_proto::google::protobuf::Timestamp;
    use tracing::{info, trace};

    use crate::comet_bft::abci::BlockWithContext;

    /// Builds and executes a new block with the given transactions, on the provided [Executor].
    ///
    /// This returns bundle state, block, and gas used.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_and_execute<BF, DB>(
        transactions: Vec<TransactionSigned>,
        chain_spec: Arc<ChainSpec>,
        block_builder_address: &Address,
        evm_config: EthEvmConfig,
        database_provider: &ProviderFactory<DB>,
        bitcoind_factory: &BF,
        bitcoin_network: bitcoin::Network,
        bitcoin_checkpoint_block_hash: &bitcoin::BlockHash,
        agg_pk: &secp256k1::PublicKey,
        timestamp: Timestamp,
    ) -> Result<BlockWithContext, BlockExecutionError>
    where
        BF: BitcoindFactory + Clone + Unpin + 'static,
        DB: Database,
    {
        // Construct block and header
        let header = build_header_template(
            &transactions,
            &database_provider,
            bitcoin_checkpoint_block_hash,
            chain_spec.clone(),
            agg_pk,
            timestamp,
            block_builder_address,
        )?;

        let mut block =
            Block { header, body: transactions, ommers: vec![], withdrawals: None, requests: None };
        let senders = TransactionSigned::recover_signers(&block.body, block.body.len())
            .ok_or(BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError))?;

        let block_with_senders =
            BlockWithSenders::new(block.clone(), senders.clone()).expect("senders are valid");

        trace!(target: "consensus::authority", transactions=?&block.body, "executing transactions");

        info!(target: "consensus::authority", "block_builder_address: {:?}", block_builder_address);
        let block_exec_output = execute(
            &block_with_senders,
            &database_provider,
            Some(*block_builder_address),
            bitcoind_factory,
            bitcoin_network,
            chain_spec,
            evm_config,
        )?;

        let completed_header = complete_header(
            block_with_senders.header.clone(),
            &block_exec_output,
            block_exec_output.gas_used,
            *bitcoin_checkpoint_block_hash,
            &database_provider,
            agg_pk,
        )?;

        // Replace header with the one that is completed
        block.header = completed_header.clone();
        let sealed_block_with_senders =
            block.seal_slow().try_seal_with_senders().expect("same senders are passed above");
        let sealed_block_with_peg = SealedBlockWithPeg::new(
            sealed_block_with_senders,
            block_exec_output.pegins,
            block_exec_output.pegouts,
        );

        let exec_outcome = ExecutionOutcome::new(
            block_exec_output.state,
            block_exec_output.receipts.into(),
            completed_header.number,
            // TODO: does authority consensus need to check against this?
            vec![],
        );
        let hashed_state = exec_outcome.hash_state_slow();
        let (_state_root, trie_updates) = StateRoot::overlay_root_with_updates(
            database_provider.provider()?.tx_ref(),
            hashed_state.clone(),
        )
        .map_err(|e| BlockExecutionError::Validation(BlockValidationError::StateRoot(e)))?;

        let block_with_context =
            BlockWithContext { sealed_block_with_peg, exec_outcome, trie_updates };

        Ok(block_with_context)
    }

    /// Fills in pre-execution header fields based on the current best block and given
    /// transactions.
    fn build_header_template<DB: Database>(
        transactions: &[TransactionSigned],
        database_provider: &ProviderFactory<DB>,
        bitcoin_checkpoint: &bitcoin::BlockHash,
        chain_spec: Arc<ChainSpec>,
        agg_pk: &secp256k1::PublicKey,
        timestamp: Timestamp,
        block_builder_address: &Address,
    ) -> Result<Header, BlockExecutionError> {
        let client = database_provider.provider()?;
        let best_block = client.best_block_number().map_err(|e| {
            BlockExecutionError::Internal(InternalBlockExecutionError::LatestBlock(e))
        })?;
        let best_hash = client
            .block_hash(best_block)
            .map_err(|e| {
                BlockExecutionError::Internal(InternalBlockExecutionError::LatestBlock(e))
            })?
            .unwrap_or_else(|| {
                panic!("best block hash not found for block number: {}", best_block);
            });
        let timestamp = timestamp.seconds as u64;

        // check previous block for base fee
        let base_fee_per_gas = client
            .header_by_hash_or_number(BlockHashOrNumber::Number(best_block))
            .expect("header to exist")
            .and_then(|parent| {
                parent.next_block_base_fee(chain_spec.base_fee_params_at_timestamp(timestamp))
            });

        // copied from `build_header_template` in autoseal
        let blob_gas_used = if chain_spec.is_cancun_active_at_timestamp(timestamp) {
            let mut sum_blob_gas_used = 0;
            for tx in transactions {
                if let Some(blob_tx) = tx.transaction.as_eip4844() {
                    sum_blob_gas_used += blob_tx.blob_gas();
                }
            }
            Some(sum_blob_gas_used)
        } else {
            None
        };

        // Construct [ExtraDataHeader] with the bitcoin checkpoint and aggregated public key
        // so the botanix consensus package can be constructed from the EDH
        let edh = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION_1,
            CHAIN_VERSION,
            *bitcoin_checkpoint,
            *agg_pk,
            *block_builder_address,
        );
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
            blob_gas_used,
            excess_blob_gas: None,
            extra_data: Bytes::from(edh.serialize()),
            parent_beacon_block_root: None,
            requests_root: None,
        };

        // copied from `build_header_template` in autoseal
        if chain_spec.is_cancun_active_at_timestamp(timestamp) {
            let parent = client.header(&best_hash).expect("header to be found");
            header.parent_beacon_block_root =
                parent.clone().and_then(|parent| parent.parent_beacon_block_root);
            header.blob_gas_used = Some(0);

            let (parent_excess_blob_gas, parent_blob_gas_used) = match parent {
                Some(parent_block)
                    if chain_spec.is_cancun_active_at_timestamp(parent_block.timestamp) =>
                {
                    (
                        parent_block.excess_blob_gas.unwrap_or_default(),
                        parent_block.blob_gas_used.unwrap_or_default(),
                    )
                }
                _ => (0, 0),
            };
            header.excess_blob_gas =
                Some(calculate_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used))
        }

        header.transactions_root = if transactions.is_empty() {
            EMPTY_TRANSACTIONS
        } else {
            proofs::calculate_transaction_root(transactions)
        };

        Ok(header)
    }

    /// Fills in the post-execution header fields based on the given PostState and gas used.
    /// In doing this, the state root is calculated and the final header is returned.
    #[allow(clippy::too_many_arguments)]
    fn complete_header<DB: Database>(
        mut header: Header,
        block_exec_result: &BlockExecutionOutput<Receipt>,
        gas_used: u64,
        recent_block_hash: bitcoin::BlockHash,
        database_provider: &ProviderFactory<DB>,
        agg_pk: &secp256k1::PublicKey,
    ) -> Result<Header, BlockExecutionError> {
        let exec_outcome = ExecutionOutcome::new(
            block_exec_result.state.clone(),
            block_exec_result.receipts.clone().into(),
            header.number,
            vec![Requests(block_exec_result.requests.clone())],
        );
        let receipts = exec_outcome.receipts_by_block(header.number);
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
        let provider = database_provider.provider()?;
        let state_root = provider
            .state_provider_by_block_number(header.number - 1)?
            .state_root(&block_exec_result.state)?;
        header.state_root = state_root;

        let block_producer_address = header.block_producer_address().map_err(|_| {
            BlockExecutionError::Validation(BlockValidationError::FailedToFetchBlockProducerAddress)
        })?;
        // Construct [ExtraDataHeader] and sign the block
        let edh = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION_1,
            CHAIN_VERSION,
            recent_block_hash,
            *agg_pk,
            block_producer_address,
        );
        header.extra_data = Bytes::from(edh.serialize());
        Ok(header)
    }

    pub(crate) fn batch_execute<DB, EF>(
        blocks: Vec<BlockWithSenders>,
        database_provider: &ProviderFactory<DB>,
        executor_factory: EF,
    ) -> Result<ExecutionOutcome, BlockExecutionError>
    where
        DB: Database,
        EF: BlockExecutorProvider,
    {
        // Assuming blocks are sorted
        if blocks.is_empty() {
            return Err(BlockExecutionError::msg("cannot execute empty batch"));
        }

        let starting_block_number = blocks.first().expect("checked above").number;
        let ending_block_number = blocks.last().expect("checked above").number;
        let provider = database_provider
            .provider()?
            .state_provider_by_block_number(starting_block_number - 1)?;
        let db = State::builder()
            .with_database_boxed(Box::new(StateProviderDatabase::new(provider)))
            .with_bundle_update()
            .build();
        let mut executor = executor_factory.batch_executor(db);

        executor.set_tip(ending_block_number);
        // TODO: set prune modes on executor
        let out = executor.execute_and_verify_batch(
            blocks.iter().map(|b| BlockExecutionInput::new(b, U256::ZERO)),
        )?;

        Ok(out)
    }

    /// Executes the block with the given block and senders, on the provided [Executor].
    ///
    /// This returns the poststate from execution and post-block changes, as well as the gas used.
    fn execute<BF, DB>(
        block: &BlockWithSenders,
        database_provider: &ProviderFactory<DB>,
        _block_builder_address: Option<Address>,
        bitcoind_factory: &BF,
        bitcoin_network: bitcoin::Network,
        chain_spec: Arc<ChainSpec>,
        evm_config: EthEvmConfig,
    ) -> Result<BlockExecutionOutput<Receipt>, BlockExecutionError>
    where
        BF: BitcoindFactory + Clone + Unpin + 'static,
        DB: Database,
    {
        // We cannot call `execute_and_verify_receipt()` here as we dont know the gas used yet
        // We must set those values on the executor after the execution
        // This is only an execution for the block builder, all other executing operations
        // should use `execute_and_verify_receipt`
        let provider =
            database_provider.provider()?.state_provider_by_block_number(block.number - 1)?;

        let db = State::builder()
            .with_database_boxed(Box::new(StateProviderDatabase::new(provider)))
            .with_bundle_update()
            .build();

        let executor = EthBlockExecutor::new(
            chain_spec,
            evm_config,
            db,
            bitcoind_factory.clone(),
            bitcoin_network,
        );
        let input = BlockExecutionInput::new(block, U256::ZERO);
        let exec_results = executor.execute(input)?;
        Ok(exec_results)
    }
}
