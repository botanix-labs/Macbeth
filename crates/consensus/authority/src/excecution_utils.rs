pub(crate) mod authority_execution_utils {
    use bitcoin::hashes::{sha256, Hash};
    use reth_btc_wallet::bitcoind::BitcoindFactory;
    use reth_consensus::Consensus;
    use reth_consensus_common::utils::{
        get_block_producer_address, unix_timestamp, validate_extra_data_header_authorities,
    };
    use reth_execution_errors::{BlockExecutionError, BlockValidationError};
    use reth_chainspec::ChainSpec;
    use reth_node_ethereum::EthEvmConfig;
    use reth_primitives::{
        constants::{EMPTY_RECEIPTS, EMPTY_TRANSACTIONS, ETHEREUM_BLOCK_GAS_LIMIT},
        extra_data_header::{ExtraDataHeader, CHAIN_VERSION, EXTRA_HEADER_VERSION},
        header_ext::HeaderExt,
        proofs, public_key_to_address, Address, Block, BlockBody, BlockHashOrNumber,
        BlockWithSenders, Bloom, Bytes, Header, ReceiptWithBloom, SealedBlock,
        SealedHeader, TransactionSigned, EMPTY_OMMER_ROOT_HASH, U256,
    };
    use reth_provider::{
        BlockExecutor, BlockReaderIdExt, BundleStateWithReceipts, ProviderError, StateProviderFactory
    };
    use reth_revm::{
        database::StateProviderDatabase, db::{states::bundle_state::BundleRetention, State},
        processor::EVMProcessor,
    };
    use std::sync::Arc;

    use tracing::{error, info, trace, warn};

    use crate::AuthorityConsensus;

    /// Builds and executes a new block with the given transactions, on the provided [Executor].
    ///
    /// This returns bundle state, block, and gas used.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_and_execute(
        transactions: Vec<TransactionSigned>,
        chain_spec: Arc<ChainSpec>,
        sk: &secp256k1::SecretKey,
        evm_config: EthEvmConfig,
        client: &(impl BlockReaderIdExt + StateProviderFactory),
        bitcoind_factory: &impl BitcoindFactory,
        bitcoin_network: bitcoin::Network,
        bitcoin_checkpoint: &bitcoin::BlockHash,
        agg_pk: &secp256k1::PublicKey,
    ) -> Result<(BundleStateWithReceipts, Block, u64), BlockExecutionError> {
        // Construct block and header
        let header = build_header_template(
            &transactions,
            client,
            bitcoin_checkpoint,
            chain_spec.clone(),
            agg_pk,
        )?;

        let block = Block { header, body: transactions, ommers: vec![], withdrawals: None, requests: None, };
        let senders = TransactionSigned::recover_signers(&block.body, block.body.len())
            .ok_or(BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError))?;

        let block_with_senders =
            BlockWithSenders::new(block.clone(), senders.clone()).expect("senders are valid");

        trace!(target: "consensus::authority", transactions=?&block.body, "executing transactions");

        // derive block builder address to receive block fees
        let block_builder_pub_key = secp256k1::PublicKey::from_secret_key_global(sk);
        let block_builder_address = public_key_to_address(block_builder_pub_key);
        info!(target: "consensus::authority", "block_builder_address: {:?}", block_builder_address);
        let (bundle_state, gas_used) = execute(
            &block_with_senders,
            client,
            Some(block_builder_address),
            bitcoind_factory,
            bitcoin_network,
            chain_spec,
            evm_config,
        )?;

        Ok((bundle_state, block, gas_used))
    }

    /// Builds and validates the current block header with the given transactions, on the provided
    /// [Executor].
    ///
    /// This returns the current block header.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_and_validate_completed_header(
        bundle_state: &BundleStateWithReceipts,
        block: Block,
        gas_used: u64,
        bitcoin_checkpoint: &bitcoin::BlockHash,
        sk: &secp256k1::SecretKey,
        authority_signers: &Vec<secp256k1::PublicKey>,
        witness_data: &Option<Vec<bitcoin::witness::Witness>>,
        utxo_commitment: sha256::Hash,
        consensus: &AuthorityConsensus,
        client: &(impl BlockReaderIdExt + StateProviderFactory),
        agg_pk: &secp256k1::PublicKey,
        genesis_authorities: &Vec<secp256k1::PublicKey>,
    ) -> Result<SealedHeader, BlockExecutionError> {
        let Block { header, body, .. } = block;
        let body = BlockBody { transactions: body, ommers: vec![], withdrawals: None, requests: None, };

        // fill in the rest of the fields
        let header = complete_header(
            header,
            bundle_state,
            gas_used,
            sk,
            witness_data,
            bitcoin_checkpoint.clone(),
            utxo_commitment,
            client,
            agg_pk,
            &authority_signers,
        )?;

        // Validate EDH authorities match genesis authorities
        if let Err(e) = validate_extra_data_header_authorities(&header, genesis_authorities) {
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
        client: &(impl BlockReaderIdExt + StateProviderFactory),
        executor_factory: &impl ExecutorFactory,
        // This is an option because the block fetcher may not be an authority
        agg_pk: Option<&secp256k1::PublicKey>,
        authorities: &Vec<secp256k1::PublicKey>,
        genesis_authorities: &Vec<secp256k1::PublicKey>,
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
                &authorities,
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
        executor.execute_and_verify_receipt(&block_with_senders, U256::ZERO)?;
        let bundle_state = executor.take_output_state();

        Ok(bundle_state)
    }

    /// Fills in pre-execution header fields based on the current best block and given
    /// transactions.
    fn build_header_template(
        transactions: &[TransactionSigned],
        client: &impl BlockReaderIdExt,
        bitcoin_checkpoint: &bitcoin::BlockHash,
        chain_spec: Arc<ChainSpec>,
        agg_pk: &secp256k1::PublicKey,
    ) -> Result<Header, BlockExecutionError> {
        let best_block = client.best_block_number().map_err(BlockExecutionError::LatestBlock)?;
        let best_hash = client
            .block_hash(best_block)
            .map_err(BlockExecutionError::LatestBlock)?
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

        // Construct [ExtraDataHeader] with the bitcoin checkpoint and aggregated public key
        // so the botanix consensus package can be constructed from the EDH
        let edh = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            CHAIN_VERSION,
            // This will be filled out complete_header
            None,
            // This will be filled out complete_header
            None,
            None,
            // This will be filled out complete_header
            None,
            *bitcoin_checkpoint,
            sha256::Hash::all_zeros(),
            *agg_pk,
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
            requests_root: None,
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
        sk: &secp256k1::SecretKey,
        witness_data: &Option<Vec<bitcoin::witness::Witness>>,
        recent_block_hash: bitcoin::BlockHash,
        utxo_commitment: sha256::Hash,
        client: &(impl BlockReaderIdExt + StateProviderFactory),
        agg_pk: &secp256k1::PublicKey,
        authorities: &Vec<secp256k1::PublicKey>,
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
            EXTRA_HEADER_VERSION,
            CHAIN_VERSION,
            // block signatures do not effect the block hash and are added after the pbft consensus
            // rounds are completed
            None,
            if header.is_poa_epoch() { Some(authorities.clone()) } else { None },
            None,
            witness_data.clone(),
            recent_block_hash,
            utxo_commitment,
            *agg_pk,
        );
        header.extra_data = Bytes::from(edh.serialize());
        header.sign_block(&sk).map_err(|e| {
            warn!(target: "consensus::authority", "failed to sign block: {:?}", e);
            BlockExecutionError::Validation(BlockValidationError::InvalidExtraData)
        })?;
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
