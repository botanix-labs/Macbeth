pub(crate) mod authority_execution_utils {
    use bitcoin::hashes::{sha256, Hash};
    use reth_btc_wallet::bitcoind::BitcoindFactory;
    use reth_consensus::Consensus;
    use reth_consensus_common::utils::{get_block_producer_address};
    use reth_interfaces::{
        executor::{BlockExecutionError, BlockValidationError},
        provider::ProviderError,
    };

    use reth_node_ethereum::EthEvmConfig;
    use reth_primitives::{
        botanix::block_with_peg::SealedBlockWithPeg,
        constants::{EMPTY_RECEIPTS, EMPTY_TRANSACTIONS, ETHEREUM_BLOCK_GAS_LIMIT},
        extra_data_header::{ExtraDataHeader, CHAIN_VERSION, EXTRA_HEADER_VERSION},
        header_ext::HeaderExt,
        proofs, Address, Block, BlockHashOrNumber, BlockWithSenders, Bloom, Bytes,
        ChainSpec, Header, ReceiptWithBloom, SealedBlock,
        TransactionSigned, EMPTY_OMMER_ROOT_HASH, U256,
    };
    use reth_provider::{
        BlockExecutor, BlockReaderIdExt, BundleStateWithReceipts, ExecutorFactory,
        StateProviderFactory,
    };
    use reth_revm::{
        database::StateProviderDatabase, db::states::bundle_state::BundleRetention,
        processor::EVMProcessor, State,
    };
    use std::sync::Arc;
    use tendermint_proto::google::protobuf::Timestamp;

    use tracing::{info, trace, warn};

    use crate::AuthorityConsensus;

    /// Builds and executes a new block with the given transactions, on the provided [Executor].
    ///
    /// This returns bundle state, block, and gas used.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_and_execute(
        transactions: Vec<TransactionSigned>,
        chain_spec: Arc<ChainSpec>,
        block_builder_address: &Address,
        evm_config: EthEvmConfig,
        client: &(impl BlockReaderIdExt + StateProviderFactory),
        bitcoind_factory: &impl BitcoindFactory,
        bitcoin_network: bitcoin::Network,
        bitcoin_checkpoint_block_hash: &bitcoin::BlockHash,
        agg_pk: &secp256k1::PublicKey,
        authority_signers: &Vec<secp256k1::PublicKey>,
        timestamp: Timestamp,
    ) -> Result<SealedBlockWithPeg, BlockExecutionError> {
        // Construct block and header
        let header = build_header_template(
            &transactions,
            client,
            bitcoin_checkpoint_block_hash,
            chain_spec.clone(),
            agg_pk,
            timestamp,
            block_builder_address,
        )?;

        let mut block = Block { header, body: transactions, ommers: vec![], withdrawals: None };
        let senders = TransactionSigned::recover_signers(&block.body, block.body.len())
            .ok_or(BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError))?;

        let block_with_senders =
            BlockWithSenders::new(block.clone(), senders.clone()).expect("senders are valid");

        trace!(target: "consensus::authority", transactions=?&block.body, "executing transactions");

        info!(target: "consensus::authority", "block_builder_address: {:?}", block_builder_address);
        let (bundle_state, gas_used) = execute(
            &block_with_senders,
            client,
            Some(*block_builder_address),
            bitcoind_factory,
            bitcoin_network,
            chain_spec,
            evm_config,
        )?;
        // Now that we have the gas used, we can complete the header
        // TODO pegin / pegout info should be stored in bundle state from here on
        let completed_header = complete_header(
            block_with_senders.header.clone(),
            &bundle_state,
            gas_used,
            // Witness Data
            &None,
            *bitcoin_checkpoint_block_hash,
            // UTXO commitment
            sha256::Hash::all_zeros(),
            client,
            agg_pk,
            &authority_signers,
        )?;

        // Replace header with the one that is completed
        block.header = completed_header.clone();
        // Seal the block
        let sealed_block = block.seal(completed_header.hash_slow());
        // TODO handle unwrap
        let sealed_block_with_senders = sealed_block.try_seal_with_senders().unwrap();

        let sealed_block_with_peg = SealedBlockWithPeg::new(
            sealed_block_with_senders.clone(),
            bundle_state.pegins().to_vec(),
            bundle_state.pegouts().to_vec(),
        );

        Ok(sealed_block_with_peg)
    }

    /// Execute and run poa validation on the block without inserting it into the storage
    /// Currently un-used
    pub(crate) fn execute_imported_block(
        consensus: &AuthorityConsensus,
        sealed_block: SealedBlock,
        client: &(impl BlockReaderIdExt + StateProviderFactory),
        executor_factory: &impl ExecutorFactory,
        // This is an option because the block fetcher may not be an authority
        agg_pk: Option<&secp256k1::PublicKey>,
        _authorities: &Vec<secp256k1::PublicKey>,
        genesis_authorities: &Vec<secp256k1::PublicKey>,
    ) -> Result<SealedBlockWithPeg, BlockExecutionError> {
        trace!(target: "consensus::authority", transactions=?&sealed_block.body, "executing transactions");
        let senders =
            TransactionSigned::recover_signers(&sealed_block.body, sealed_block.body.len()).ok_or(
                BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError),
            )?;

        let sealed_block_with_senders =
            BlockWithSenders::new(sealed_block.clone().unseal(), senders.clone())
                .expect("senders are valid");

        // validate before executing block
        // Edge case: block 1 for the rpc nodes
        // Rpc nodes will typically store the agg pk from the latest block on boot up
        // In the case where they boot up on block 0, they will not have an agg pk
        // Here we pull the agg pk from the incoming block if it is not provided
        let aggregate_public_key = {
            if let Some(current_pk) = agg_pk {
                current_pk.clone()
            } else {
                let current_agg_key =
                    sealed_block.header.clone().unseal().get_aggregate_public_key().map_err(
                        |_e| {
                            BlockExecutionError::Validation(BlockValidationError::InvalidExtraData)
                        },
                    )?;
                current_agg_key.clone()
            }
        };

        consensus
            .validate_header_standalone(
                &sealed_block.header.clone(),
                &genesis_authorities,
                // TODO(https://github.com/botanix-labs/botanix/issues/615) this shouldn't need to be an option
                Some(&aggregate_public_key),
            )
            .map_err(|e| {
                warn!(target: "consensus::authority", "failed to validate POA header: {:?}", e);
                // TODO(armins) return more expressive error
                BlockExecutionError::Validation(BlockValidationError::InvalidExtraData)
            })?;

        let _block_builder_address = get_block_producer_address(&sealed_block.header.clone());
        let db = client.latest().map_err(|e| BlockExecutionError::LatestBlock(e))?;
        let mut executor = executor_factory.with_state(db);
        executor.execute_and_verify_receipt(&sealed_block_with_senders.clone(), U256::ZERO)?;
        let bundle_state = executor.take_output_state();
        let sealed_block_with_peg = SealedBlockWithPeg::new(
            sealed_block_with_senders.seal_slow(),
            bundle_state.pegins().to_vec(),
            bundle_state.pegouts().to_vec(),
        );

        Ok(sealed_block_with_peg)
    }

    /// Fills in pre-execution header fields based on the current best block and given
    /// transactions.
    fn build_header_template(
        transactions: &[TransactionSigned],
        client: &impl BlockReaderIdExt,
        bitcoin_checkpoint: &bitcoin::BlockHash,
        chain_spec: Arc<ChainSpec>,
        agg_pk: &secp256k1::PublicKey,
        timestamp: Timestamp,
        block_builder_address: &Address,
    ) -> Result<Header, BlockExecutionError> {
        let best_block = client.best_block_number().map_err(BlockExecutionError::LatestBlock)?;
        let best_hash = client
            .block_hash(best_block)
            .map_err(BlockExecutionError::LatestBlock)?
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

        // Construct [ExtraDataHeader] with the bitcoin checkpoint and aggregated public key
        // so the botanix consensus package can be constructed from the EDH
        let edh = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            CHAIN_VERSION,
            *bitcoin_checkpoint,
            *agg_pk,
            block_builder_address.clone(),
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
            blob_gas_used: None,
            excess_blob_gas: None,
            extra_data: Bytes::from(edh.serialize()),
            parent_beacon_block_root: None,
        };

        // TODO (armins) Poa shouldnt be minging empty blocks
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
    fn complete_header(
        mut header: Header,
        bundle_state: &BundleStateWithReceipts,
        gas_used: u64,
        _witness_data: &Option<Vec<bitcoin::witness::Witness>>,
        recent_block_hash: bitcoin::BlockHash,
        _utxo_commitment: sha256::Hash,
        client: &(impl BlockReaderIdExt + StateProviderFactory),
        agg_pk: &secp256k1::PublicKey,
        _authorities: &Vec<secp256k1::PublicKey>,
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

        // TODO remove this unwrap
        let block_producer_address = header.block_producer_address().unwrap();
        // Construct [ExtraDataHeader] and sign the block
        let edh = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            CHAIN_VERSION,
            recent_block_hash,
            *agg_pk,
            block_producer_address,
        );
        header.extra_data = Bytes::from(edh.serialize());
        Ok(header)
    }

    /// Executes the block with the given block and senders, on the provided [Executor].
    ///
    /// This returns the poststate from execution and post-block changes, as well as the gas used.
    fn execute(
        block: &BlockWithSenders,
        client: &(impl StateProviderFactory + BlockReaderIdExt),
        block_builder_address: Option<Address>,
        bitcoind_factory: &impl BitcoindFactory,
        bitcoin_network: bitcoin::Network,
        chain_spec: Arc<ChainSpec>,
        evm_config: EthEvmConfig,
    ) -> Result<(BundleStateWithReceipts, u64), BlockExecutionError> {
        // We cannot call `execute_and_verify_receipt()` here as we dont know the gas used yet
        // We must set those values on the executor after the execution
        // This is only an execution for the block builder, all other executing operations
        // should use `execute_and_verify_receipt`
        let db = State::builder()
            .with_database_boxed(Box::new(StateProviderDatabase::new(client.latest().unwrap())))
            .with_bundle_update()
            .build();

        let mut executor = EVMProcessor::new_with_state(chain_spec, db, evm_config);
        // This step is typically done by the EVMExecutorFactory
        executor.with_bitcoind_factory(bitcoind_factory.clone(), bitcoin_network);

        // The following is cloning what is done in execute.inner() in the processor
        // set the first block to find the correct index in bundle state
        executor.set_first_block(block.number);

        let (receipts, gas_used, total_block_fees) =
            executor.execute_transactions(block, U256::ZERO)?;

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

        Ok((executor.take_output_state(), gas_used))
    }
}
