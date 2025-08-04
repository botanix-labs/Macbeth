//! Ethereum block executor.

use crate::{
    dao_fork::{DAO_HARDFORK_BENEFICIARY, DAO_HARDKFORK_ACCOUNTS},
    EthEvmConfig,
};
use botanix_authority_edh::header_ext::HeaderExt;
use botanix_authority_peg::{
    consensus_package::BotanixConsensusPackage,
    mint_validation::{try_parse_burn_event, try_parse_mint_event, MintContractError},
    peg_contract::{PeginData, PegoutWithId},
};
use botanix_btc_wallet::{
    bitcoind::{BitcoindConfig, BitcoindFactory},
    test_utils::MockBitcoindFactory,
};
use btcserverlib::pegout_id::PegoutId;
use core::fmt::Display;
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_db::{test_utils::TempDatabase, DatabaseEnv};
use reth_ethereum_consensus::validate_block_post_execution;
use reth_evm::{
    execute::{
        BatchExecutor, BlockExecutionError, BlockExecutionInput, BlockExecutionOutput,
        BlockExecutorProvider, BlockValidationError, Executor, InternalBlockExecutionError,
        ProviderError,
    },
    system_calls::{
        apply_beacon_root_contract_call, apply_consolidation_requests_contract_call,
        apply_withdrawal_requests_contract_call,
    },
    ConfigureEvm,
};
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{
    Address, BlockNumber, BlockWithSenders, EthereumHardfork, Header, Receipt, Request, TxHash,
    U256,
};
use reth_provider::{
    test_utils::create_test_provider_factory, BlockReader, DatabaseProviderFactory,
    DatabaseProviderRO,
};
use reth_prune_types::PruneModes;
use reth_revm::{
    batch::BlockBatchRecord,
    db::states::bundle_state::BundleRetention,
    state_change::{apply_blockhashes_update, post_block_balance_increments},
    Evm, State,
};
use revm_primitives::{
    db::{Database, DatabaseCommit},
    hex, Account, BlockEnv, CfgEnvWithHandlerCfg, EVMError, EnvWithHandlerCfg, ExecutionResult,
    ResultAndState,
};
use std::collections::HashMap;
use tracing::{error, info};

#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};
#[cfg(feature = "std")]
use std::sync::Arc;

/// Provides executors to execute regular ethereum blocks
#[derive(Debug, Clone)]
pub struct EthExecutorProvider<BF, RethDB, EvmConfig = EthEvmConfig>
where
    RethDB: reth_db::Database,
{
    chain_spec: Arc<ChainSpec>,
    evm_config: EvmConfig,
    bitcoind_factory: BF,
    bitcoin_network: bitcoin::Network,
    provider: Arc<DatabaseProviderRO<RethDB>>,
}

/// Create a noop executor provider with chain spec
pub fn create_noop_executor_provider(
    chain_spec: Arc<ChainSpec>,
) -> EthExecutorProvider<MockBitcoindFactory, Arc<TempDatabase<DatabaseEnv>>, EthEvmConfig> {
    EthExecutorProvider::new(
        chain_spec,
        EthEvmConfig::default(),
        MockBitcoindFactory::new(BitcoindConfig::default()),
        bitcoin::Network::Regtest,
        Arc::new(create_test_provider_factory().database_provider_ro().unwrap()),
    )
}

impl<BF, RethDB> EthExecutorProvider<BF, RethDB>
where
    RethDB: reth_db::Database,
{
    /// Creates a new default ethereum executor provider.
    pub fn ethereum(
        chain_spec: Arc<ChainSpec>,
        bitcoind_factory: BF,
        bitcoin_network: bitcoin::Network,
        provider: Arc<DatabaseProviderRO<RethDB>>,
    ) -> Self {
        Self::new(chain_spec, Default::default(), bitcoind_factory, bitcoin_network, provider)
    }
}

impl<BF, RethDB, EvmConfig> EthExecutorProvider<BF, RethDB, EvmConfig>
where
    RethDB: reth_db::Database,
{
    /// Creates a new executor provider.
    pub const fn new(
        chain_spec: Arc<ChainSpec>,
        evm_config: EvmConfig,
        bitcoind_factory: BF,
        bitcoin_network: bitcoin::Network,
        provider: Arc<DatabaseProviderRO<RethDB>>,
    ) -> Self {
        Self { chain_spec, evm_config, bitcoind_factory, bitcoin_network, provider }
    }
}

impl<BF, RethDB, EvmConfig> EthExecutorProvider<BF, RethDB, EvmConfig>
where
    BF: BitcoindFactory + Clone + Unpin + 'static,
    EvmConfig: ConfigureEvm,
    RethDB: reth_db::Database,
{
    fn eth_executor<DB>(&self, db: DB) -> EthBlockExecutor<EvmConfig, DB, BF, RethDB>
    where
        DB: Database<Error: Into<ProviderError>>,
    {
        EthBlockExecutor::new(
            self.chain_spec.clone(),
            self.evm_config.clone(),
            State::builder().with_database(db).with_bundle_update().without_state_clear().build(),
            self.bitcoind_factory.clone(),
            self.bitcoin_network,
            self.provider.clone(),
        )
    }
}

impl<BF, RethDB, EvmConfig> BlockExecutorProvider for EthExecutorProvider<BF, RethDB, EvmConfig>
where
    BF: BitcoindFactory + Clone + Unpin + 'static,
    EvmConfig: ConfigureEvm,
    RethDB: reth_db::Database + Clone + 'static,
{
    type Executor<DB: Database<Error: Into<ProviderError> + Display>> =
        EthBlockExecutor<EvmConfig, DB, BF, RethDB>;

    type BatchExecutor<DB: Database<Error: Into<ProviderError> + Display>> =
        EthBatchExecutor<EvmConfig, DB, BF, RethDB>;

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        self.eth_executor(db)
    }

    fn batch_executor<DB>(&self, db: DB) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        let executor = self.eth_executor(db);
        EthBatchExecutor { executor, batch_record: BlockBatchRecord::default() }
    }
}

/// Helper type for the output of executing a block.
#[derive(Debug, Clone)]
struct EthExecuteOutput {
    receipts: Vec<Receipt>,
    requests: Vec<Request>,
    gas_used: u64,
    total_block_fees: u128,
    pegins: Vec<PeginData>,
    pegouts: Vec<PegoutWithId>,
}

/// Helper container type for EVM with chain spec.
#[derive(Debug, Clone)]
struct EthEvmExecutor<EvmConfig, BF, RethDB>
where
    RethDB: reth_db::Database,
{
    /// The chainspec
    chain_spec: Arc<ChainSpec>,
    /// How to create an EVM.
    evm_config: EvmConfig,
    /// The bitcoind factory used to connect to the L1 bitcoind RPC
    bitcoind_factory: BF,
    /// The L1 bitcoin network
    bitcoin_network: bitcoin::Network,
    /// Blockchain provider
    provider: Arc<DatabaseProviderRO<RethDB>>,
}

impl<EvmConfig, BF, RethDB> EthEvmExecutor<EvmConfig, BF, RethDB>
where
    EvmConfig: ConfigureEvm,
    BF: BitcoindFactory + Clone + Unpin + 'static,
    RethDB: reth_db::Database,
{
    /// Executes the transactions in the block and returns the receipts of the transactions in the
    /// block, the total gas used and the list of EIP-7685 [requests](Request).
    /// As well as pegins and pegouts
    ///
    /// This applies the pre-execution and post-execution changes that require an [EVM](Evm), and
    /// executes the transactions.
    ///
    /// # Note
    ///
    /// It does __not__ apply post-execution changes that do not require an [EVM](Evm), for that see
    /// [`EthBlockExecutor::post_execution`].
    fn execute_state_transitions<Ext, DB>(
        &self,
        block: &BlockWithSenders,
        mut evm: Evm<'_, Ext, &mut State<DB>>,
        botanix_consensus_pkg: BotanixConsensusPackage,
        provider: Arc<DatabaseProviderRO<RethDB>>,
    ) -> Result<EthExecuteOutput, BlockExecutionError>
    where
        DB: Database,
        DB::Error: Into<ProviderError> + Display,
    {
        // apply pre execution changes
        apply_beacon_root_contract_call(
            &self.evm_config,
            &self.chain_spec,
            block.timestamp,
            block.number,
            block.parent_beacon_block_root,
            &mut evm,
        )?;
        apply_blockhashes_update(
            evm.db_mut(),
            &self.chain_spec,
            block.timestamp,
            block.number,
            block.parent_hash,
        )?;

        // execute transactions
        let mut total_pegins = vec![];
        let mut total_pegouts = vec![];

        let mut total_block_fees = 0_u128;
        let mut cumulative_gas_used = 0;
        let base_fee = block.base_fee_per_gas;
        let mut receipts = Vec::with_capacity(block.body.len());

        for (sender, transaction) in block.transactions_with_sender() {
            // The sum of the transaction’s gas limit, Tg, and the gas utilized in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = block.header.gas_limit - cumulative_gas_used;
            if transaction.gas_limit() > block_available_gas {
                return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                }
                .into());
            }

            self.evm_config.fill_tx_env(evm.tx_mut(), transaction, *sender);

            // Store original sender account info before transaction execution.
            // This is used later to partially revert the state if botanix specific validation
            // fails.
            // If the sender is not found in the state, we need to error because there is no balance
            // to subtract from. This shouldn't happen because the tx would have failed.
            let sender_db_error =
                BlockExecutionError::Internal(InternalBlockExecutionError::Other(
                    format!("DB error getting sender: {}", hex::encode(sender)).into(),
                ));
            let sender_not_found =
                BlockExecutionError::Internal(InternalBlockExecutionError::Other(
                    format!("Sender not found in state: {}", hex::encode(sender)).into(),
                ));
            let mut original_sender_info = evm
                .db_mut()
                .basic(*sender)
                .map_err(|_| sender_db_error)?
                .ok_or(sender_not_found)?;

            // Execute transaction.
            let ResultAndState { mut result, mut state } = evm.transact().map_err(move |err| {
                let new_err = match err {
                    EVMError::Transaction(e) => EVMError::Transaction(e),
                    EVMError::Header(e) => EVMError::Header(e),
                    EVMError::Database(e) => EVMError::Database(e.into()),
                    EVMError::Custom(e) => EVMError::Custom(e),
                    EVMError::Precompile(e) => EVMError::Precompile(e),
                };
                // Ensure hash is calculated for error log, if not already done
                BlockValidationError::EVM {
                    hash: transaction.recalculate_hash(),
                    error: Box::new(new_err),
                }
            })?;

            // calculate the total transaction fee
            let mut transaction_fee =
                transaction.clone().effective_tip_per_gas(base_fee).expect("base fee exists");
            // Include the base fee so it's not burned
            transaction_fee += base_fee.expect("base fee exists") as u128;
            total_block_fees += transaction_fee * u128::from(result.gas_used());

            // append gas used
            cumulative_gas_used += result.gas_used();

            // ***** Botanix specific checks ******
            let mut pegins = vec![];
            let mut pegouts = vec![];

            let new_result = {
                if result.is_success() {
                    match self.botanix_mint_contract_checks(
                        &result,
                        &botanix_consensus_pkg,
                        transaction.hash,
                        provider.clone(),
                    ) {
                        Ok((new_pegins, new_pegouts)) => {
                            pegins.extend(new_pegins);
                            pegouts.extend(new_pegouts);
                            result
                        }
                        Err(e) => {
                            info!("Botanix Minting contract event validation failed: {:?}", e);

                            // Capture gas used from the initially successful execution
                            let gas_used = result.gas_used();

                            // Determine the total gas cost: gas_used * effective_gas_price
                            // Base fee is needed for effective_gas_price calculation
                            let effective_gas_price = transaction.effective_gas_price(base_fee);
                            let total_gas_cost =
                                U256::from(gas_used) * U256::from(effective_gas_price);

                            // Get the new nonce. This should be original nonce + 1
                            let new_nonce = state
                                .get(sender)
                                .ok_or(BlockExecutionError::Internal(
                                    InternalBlockExecutionError::Other(
                                        format!(
                                            "Sender not found in state: {}",
                                            hex::encode(sender)
                                        )
                                        .into(),
                                    ),
                                ))?
                                .info
                                .nonce;

                            // Clear ALL state changes introduced by the transaction.
                            // State is the diff of the previous state and the new state after the
                            // transaction.
                            state.clear();

                            // Now, re-apply *only* the total gas cost and nonce change to the
                            // pre-transaction state.
                            original_sender_info.nonce = new_nonce;
                            // There shouldn't be an underflow because the tx would have failed if
                            // the sender didn't have enough balance.
                            original_sender_info.balance = original_sender_info
                                .balance
                                .checked_sub(total_gas_cost)
                                .ok_or(BlockExecutionError::Internal(
                                    InternalBlockExecutionError::Other(
                                        "Sender balance underflow".to_string().into(),
                                    ),
                                ))?;

                            // Re-insert the sender's account with the original info but updated
                            // nonce and balance (reflecting only gas cost).
                            // Create new diff with only above changes.
                            let reverted_account = Account {
                                info: original_sender_info,
                                storage: HashMap::new(), // Storage changes remain reverted.
                                status: revm_primitives::AccountStatus::Touched, // Mark as touched
                            };
                            state.insert(*sender, reverted_account);

                            // Return a revert result, indicating failure but consuming gas and
                            // incrementing nonce.
                            ExecutionResult::Revert {
                                gas_used, // Still report the gas used
                                output: Default::default(),
                            }
                        }
                    }
                } else {
                    result
                }
            };

            result = new_result;

            evm.db_mut().commit(state);

            // Push transaction changeset and calculate header bloom filter for receipt.
            receipts.push(
                #[allow(clippy::needless_update)] // side-effect of optimism fields
                Receipt {
                    tx_type: transaction.tx_type(),
                    // Success flag was added in `EIP-658: Embedding transaction status code in
                    // receipts`.
                    success: result.is_success(),
                    cumulative_gas_used,
                    // convert to reth log
                    logs: result.into_logs(),
                    ..Default::default()
                },
            );
            total_pegins.extend(pegins);
            total_pegouts.extend(pegouts);
        }

        // For eip-6110 we need to collect the deposit requests. This is irrelevant for poa
        // consensus
        let requests = if self.chain_spec.is_prague_active_at_timestamp(block.timestamp) {
            // Collect all EIP-6110 deposits
            let deposit_requests =
                crate::eip6110::parse_deposits_from_receipts(&self.chain_spec, &receipts)?;

            // Collect all EIP-7685 requests
            let withdrawal_requests =
                apply_withdrawal_requests_contract_call(&self.evm_config, &mut evm)?;

            // Collect all EIP-7251 requests
            let consolidation_requests =
                apply_consolidation_requests_contract_call(&self.evm_config, &mut evm)?;

            [deposit_requests, withdrawal_requests, consolidation_requests].concat()
        } else {
            vec![]
        };

        Ok(EthExecuteOutput {
            receipts,
            requests,
            gas_used: cumulative_gas_used,
            total_block_fees,
            pegins: total_pegins,
            pegouts: total_pegouts,
        })
    }

    /// Performs additional checks on mint contract transactions.
    #[tracing::instrument(
        level = "trace",
        skip(self, result, botanix_consensus_pkg, provider),
        fields(
            bitcoin_checkpoint_hash = %botanix_consensus_pkg.bitcoin_checkpoint.0.block_hash(),
            bitcoin_checkpoint_height = botanix_consensus_pkg.bitcoin_checkpoint.1,
        )
    )]
    fn botanix_mint_contract_checks(
        &self,
        result: &ExecutionResult,
        botanix_consensus_pkg: &BotanixConsensusPackage,
        tx_hash: TxHash,
        provider: Arc<DatabaseProviderRO<RethDB>>,
    ) -> Result<(Vec<PeginData>, Vec<PegoutWithId>), MintContractError> {
        let consensus_pkg = botanix_consensus_pkg;
        let btc_network = consensus_pkg.btc_network;

        tracing::trace!("botanix_consensus_package={:?}", botanix_consensus_pkg);

        // Check pegins.
        let mut pegins = vec![];
        let mut pegouts = vec![];
        for log in result.logs() {
            let pegin_data = match try_parse_mint_event(log)? {
                None => continue,
                Some(p) => p,
            };

            tracing::trace!(?pegin_data, "validate pegin data for tx {}", tx_hash);

            // Get the reference block hash from the pegin metadata.
            // This is used to avoid the growing list of headers in the pegin metadata
            // by using a bitcoin checkpoint that is close to the pegin block height.
            // The reference block hash is only provided for version v1.
            let mut bitcoin_checkpoint = consensus_pkg.bitcoin_checkpoint;
            let (version, ref_block_hash) = if let Some(meta) = pegin_data.meta.first() {
                match (meta.version(), meta.ref_block_hash()) {
                    (1, None) => {
                        return Err(MintContractError::InvalidPeginData {
                            error: "Reference block hash cannot be found".to_string(),
                            revert_address: pegin_data.account,
                            revert_amount: pegin_data.amount,
                        })
                    }
                    (1, Some(hash)) => {
                        match provider.find_block_by_hash(hash, reth_provider::BlockSource::Any) {
                            Ok(Some(block)) => {
                                let header = block.header;
                                let package = header
                                    .botanix_consensus_package(
                                        self.bitcoin_network,
                                        self.bitcoind_factory.clone(),
                                    )
                                    .map_err(|_| MintContractError::InvalidPeginData {
                                        error: "Failed to get botanix consensus package"
                                            .to_string(),
                                        revert_address: pegin_data.account,
                                        revert_amount: pegin_data.amount,
                                    })?;
                                bitcoin_checkpoint = package.bitcoin_checkpoint;

                                tracing::debug!(
                                    pegin_meta_version = meta.version(),
                                    ref_eth_block_hash = %hash,
                                    overridden_btc_checkpoint_hash = %bitcoin_checkpoint.0.block_hash(),
                                    overridden_btc_checkpoint_height = %bitcoin_checkpoint.1,
                                    "overridden bitcoin checkpoint for V1 pegin via ref_block_hash"
                                );
                            }
                            Ok(None) => {
                                return Err(MintContractError::InvalidPeginData {
                                    error: "No block found for reference block hash".to_string(),
                                    revert_address: pegin_data.account,
                                    revert_amount: pegin_data.amount,
                                })
                            }
                            Err(_) => panic!("Database error fetching reference block hash"),
                        };
                    }
                    (0, Some(_)) => {
                        return Err(MintContractError::InvalidPeginData {
                            error: "Not expecting reference block hash in proof version 0"
                                .to_string(),
                            revert_address: pegin_data.account,
                            revert_amount: pegin_data.amount,
                        })
                    }
                    _ => {}
                };
                (meta.version(), meta.ref_block_hash())
            } else {
                return Err(MintContractError::InvalidPeginData {
                    error: "No proofs found in pegin data".to_string(),
                    revert_address: pegin_data.account,
                    revert_amount: pegin_data.amount,
                });
            };

            for meta in &pegin_data.meta {
                if meta.version() != version {
                    return Err(MintContractError::InvalidPeginData {
                        error: "Proofs have mismatching versions".to_string(),
                        revert_address: pegin_data.account,
                        revert_amount: pegin_data.amount,
                    });
                }

                if meta.ref_block_hash() != ref_block_hash {
                    return Err(MintContractError::InvalidPeginData {
                        error: "Proofs have mismatching reference block hashes".to_string(),
                        revert_address: pegin_data.account,
                        revert_amount: pegin_data.amount,
                    });
                }
            }

            // the pegin height must be equal or less than the required block depth (checkpoint)
            if pegin_data.bitcoin_block_height > bitcoin_checkpoint.1 {
                return Err(MintContractError::InvalidPeginData {
                    error: format!(
                        "pegin height {} greater than checkpoint of {}",
                        pegin_data.bitcoin_block_height, bitcoin_checkpoint.1,
                    ),
                    revert_address: pegin_data.account,
                    revert_amount: pegin_data.amount,
                });
            }
            let aggregate_public_key = consensus_pkg.aggregate_public_key;
            match pegin_data.validate(&bitcoin_checkpoint, &aggregate_public_key) {
                Ok(aggregate_value) => {
                    if pegin_data.amount >= aggregate_value {
                        return Err(MintContractError::InvalidPeginData {
                            error: format!(
                                "pegin amount should be less than aggregate value: \
                                    pegin aggregate value: {}; pegin amount: {}",
                                aggregate_value, pegin_data.amount,
                            ),
                            revert_address: pegin_data.account,
                            revert_amount: pegin_data.amount,
                        });
                    }

                    tracing::debug!(validated_aggregate_value = %aggregate_value, "pegin data validation succeeded");
                }
                Err(e) => {
                    tracing::debug!(error = ?e, ?pegin_data, "pegin data validation failed: {e}");
                    return Err(MintContractError::InvalidPeginData {
                        error: format!("pegin validation failed: {}", e),
                        revert_address: pegin_data.account,
                        revert_amount: pegin_data.amount,
                    });
                }
            }

            pegins.push(pegin_data);
        }

        // Check pegouts
        for (index, log) in result.logs().iter().enumerate() {
            if let Some(pegout_data) = try_parse_burn_event(log, btc_network)? {
                let mut tx_hash_array = [0u8; 32];
                tx_hash_array.copy_from_slice(tx_hash.as_slice());
                let pegout_id = PegoutId::new(tx_hash_array, index as u32);
                let pegout_with_id = PegoutWithId { data: pegout_data, id: pegout_id };
                pegouts.push(pegout_with_id);
            }
        }

        Ok((pegins, pegouts))
    }
}

/// A basic Ethereum block executor.
///
/// Expected usage:
/// - Create a new instance of the executor.
/// - Execute the block.
#[derive(Debug)]
pub struct EthBlockExecutor<EvmConfig, DB, BF, RethDB>
where
    RethDB: reth_db::Database,
{
    /// Chain specific evm config that's used to execute a block.
    executor: EthEvmExecutor<EvmConfig, BF, RethDB>,
    /// The state to use for execution
    state: State<DB>,
}

impl<EvmConfig, DB, BF, RethDB> EthBlockExecutor<EvmConfig, DB, BF, RethDB>
where
    RethDB: reth_db::Database,
{
    /// Creates a new Ethereum block executor.
    pub const fn new(
        chain_spec: Arc<ChainSpec>,
        evm_config: EvmConfig,
        state: State<DB>,
        bitcoind_factory: BF,
        bitcoin_network: bitcoin::Network,
        provider: Arc<DatabaseProviderRO<RethDB>>,
    ) -> Self {
        Self {
            executor: EthEvmExecutor {
                chain_spec,
                evm_config,
                bitcoind_factory,
                bitcoin_network,
                provider,
            },
            state,
        }
    }

    #[inline]
    fn chain_spec(&self) -> &ChainSpec {
        self.executor.chain_spec.as_ref()
    }

    /// Returns mutable reference to the state that wraps the underlying database.
    #[allow(unused)]
    fn state_mut(&mut self) -> &mut State<DB> {
        &mut self.state
    }
}

impl<EvmConfig, DB, BF, RethDB> EthBlockExecutor<EvmConfig, DB, BF, RethDB>
where
    EvmConfig: ConfigureEvm,
    DB: Database<Error: Into<ProviderError> + Display>,
    BF: BitcoindFactory + Clone + Unpin + 'static,
    RethDB: reth_db::Database,
{
    /// Configures a new evm configuration and block environment for the given block.
    ///
    /// # Caution
    ///
    /// This does not initialize the tx environment.
    fn evm_env_for_block(&self, header: &Header, total_difficulty: U256) -> EnvWithHandlerCfg {
        let mut cfg = CfgEnvWithHandlerCfg::new(Default::default(), Default::default());
        let mut block_env = BlockEnv::default();
        self.executor.evm_config.fill_cfg_and_block_env(
            &mut cfg,
            &mut block_env,
            self.chain_spec(),
            header,
            total_difficulty,
        );

        EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, Default::default())
    }

    /// Execute a single block and apply the state changes to the internal state.
    ///
    /// Returns the receipts of the transactions in the block, the total gas used and the list of
    /// EIP-7685 [requests](Request).
    ///
    /// Returns an error if execution fails.
    fn execute_without_verification(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<EthExecuteOutput, BlockExecutionError> {
        // 1. prepare state on new block
        self.on_new_block(&block.header);

        let header = block.header.clone();
        let edh = block.header.deserialize_extra_data_header().map_err(|_| {
            BlockExecutionError::Validation(BlockValidationError::ExtraDataSerializeError)
        })?;

        let botanix_consensus_pkg = header
            .botanix_consensus_package(
                self.executor.bitcoin_network,
                self.executor.bitcoind_factory.clone(),
            )
            .map_err(|e| {
                error!("Failed to get botanix consensus package: {:?}", e);
                BlockExecutionError::Validation(BlockValidationError::BotanixConsensusPkgError(e))
            })?;

        let block_fee_recipient_address = edh.block_fee_recipient_address;

        // 2. configure the evm and execute
        let env = self.evm_env_for_block(&block.header, total_difficulty);
        let output: EthExecuteOutput = {
            let evm = self.executor.evm_config.evm_with_env(&mut self.state, env);
            self.executor.execute_state_transitions(
                block,
                evm,
                botanix_consensus_pkg,
                self.executor.provider.clone(),
            )?
        };

        // 3. apply post execution changes
        self.post_execution(
            block,
            total_difficulty,
            Some(output.total_block_fees),
            block_fee_recipient_address,
        )?;

        Ok(output)
    }

    /// Apply settings before a new block is executed.
    pub(crate) fn on_new_block(&mut self, header: &Header) {
        // Set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag = self.chain_spec().is_spurious_dragon_active_at_block(header.number);
        self.state.set_state_clear_flag(state_clear_flag);
    }

    /// Apply post execution state changes that do not require an [EVM](Evm), such as: block
    /// rewards, withdrawals, and irregular DAO hardfork state change
    pub fn post_execution(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
        total_block_fees: Option<u128>,
        block_fee_recipient_address: Address,
    ) -> Result<(), BlockExecutionError> {
        let mut balance_increments = post_block_balance_increments(
            self.chain_spec(),
            block,
            total_difficulty,
            total_block_fees,
            Some(block_fee_recipient_address),
        );

        // Irregular state change at Ethereum DAO hardfork
        if self.chain_spec().fork(EthereumHardfork::Dao).transitions_at_block(block.number) {
            // drain balances from hardcoded addresses.
            let drained_balance: u128 = self
                .state
                .drain_balances(DAO_HARDKFORK_ACCOUNTS)
                .map_err(|_| BlockValidationError::IncrementBalanceFailed)?
                .into_iter()
                .sum();

            // return balance to DAO beneficiary.
            *balance_increments.entry(DAO_HARDFORK_BENEFICIARY).or_default() += drained_balance;
        }
        // increment balances
        self.state
            .increment_balances(balance_increments)
            .map_err(|_| BlockValidationError::IncrementBalanceFailed)?;

        Ok(())
    }
}

impl<EvmConfig, DB, BF, RethDB> Executor<DB> for EthBlockExecutor<EvmConfig, DB, BF, RethDB>
where
    EvmConfig: ConfigureEvm,
    DB: Database<Error: Into<ProviderError> + Display>,
    BF: BitcoindFactory + Clone + Unpin + 'static,
    RethDB: reth_db::Database,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = BlockExecutionError;

    /// Executes the block and commits the changes to the internal state.
    ///
    /// Returns the receipts of the transactions in the block.
    ///
    /// Returns an error if the block could not be executed or failed verification.
    fn execute(mut self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let BlockExecutionInput { block, total_difficulty } = input;
        let EthExecuteOutput { receipts, requests, gas_used, total_block_fees, pegins, pegouts } =
            self.execute_without_verification(block, total_difficulty)?;

        // TODO NOTE: we need to merge keep the reverts for the bundle retention
        self.state.merge_transitions(BundleRetention::Reverts);
        Ok(BlockExecutionOutput {
            state: self.state.take_bundle(),
            receipts,
            requests,
            gas_used,
            total_block_fees,
            pegins,
            pegouts,
        })
    }
}

/// An executor for a batch of blocks.
///
/// State changes are tracked until the executor is finalized.
#[derive(Debug)]
pub struct EthBatchExecutor<EvmConfig, DB, BF, RethDB>
where
    RethDB: reth_db::Database,
{
    /// The executor used to execute single blocks
    ///
    /// All state changes are committed to the [State].
    executor: EthBlockExecutor<EvmConfig, DB, BF, RethDB>,
    /// Keeps track of the batch and records receipts based on the configured prune mode
    batch_record: BlockBatchRecord,
}

impl<EvmConfig, DB, BF, RethDB> EthBatchExecutor<EvmConfig, DB, BF, RethDB>
where
    RethDB: reth_db::Database,
{
    /// Returns mutable reference to the state that wraps the underlying database.
    #[allow(unused)]
    fn state_mut(&mut self) -> &mut State<DB> {
        self.executor.state_mut()
    }
}

impl<EvmConfig, DB, BF, RethDB> BatchExecutor<DB> for EthBatchExecutor<EvmConfig, DB, BF, RethDB>
where
    EvmConfig: ConfigureEvm,
    DB: Database<Error: Into<ProviderError> + Display>,
    BF: BitcoindFactory + Clone + Unpin + 'static,
    RethDB: reth_db::Database,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = ExecutionOutcome;
    type Error = BlockExecutionError;

    fn execute_and_verify_one(&mut self, input: Self::Input<'_>) -> Result<(), Self::Error> {
        let BlockExecutionInput { block, total_difficulty } = input;

        if self.batch_record.first_block().is_none() {
            self.batch_record.set_first_block(block.number);
        }

        let EthExecuteOutput {
            receipts,
            requests,
            gas_used: _,
            total_block_fees: _,
            pegins: _,
            pegouts: _,
        } = self.executor.execute_without_verification(block, total_difficulty)?;

        validate_block_post_execution(block, self.executor.chain_spec(), &receipts, &requests)?;

        // prepare the state according to the prune mode
        let retention = self.batch_record.bundle_retention(block.number);
        self.executor.state.merge_transitions(retention);

        // store receipts in the set
        self.batch_record.save_receipts(receipts)?;

        // store requests in the set
        self.batch_record.save_requests(requests);

        Ok(())
    }

    fn finalize(mut self) -> Self::Output {
        ExecutionOutcome::new(
            self.executor.state.take_bundle(),
            self.batch_record.take_receipts(),
            self.batch_record.first_block().unwrap_or_default(),
            self.batch_record.take_requests(),
        )
    }

    fn set_tip(&mut self, tip: BlockNumber) {
        self.batch_record.set_tip(tip);
    }

    fn set_prune_modes(&mut self, prune_modes: PruneModes) {
        self.batch_record.set_prune_modes(prune_modes);
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.executor.state.bundle_state.size_hint())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::{
        eip2935::HISTORY_STORAGE_ADDRESS,
        eip4788::{BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE, SYSTEM_ADDRESS},
        eip7002::{WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE},
    };
    use botanix_authority_edh::extra_data_header::ExtraDataHeader;
    use reth_chainspec::{ChainSpecBuilder, ForkCondition, MAINNET};
    use reth_primitives::{
        constants::{EMPTY_ROOT_HASH, ETH_TO_WEI},
        keccak256, public_key_to_address, Account, Block, Transaction, TxKind, TxLegacy, B256,
    };
    use reth_revm::{
        database::StateProviderDatabase, test_utils::StateProviderTest, TransitionState,
    };
    use reth_testing_utils::generators::{self, sign_tx_with_key_pair};
    use revm_primitives::{b256, fixed_bytes, Bytes, BLOCKHASH_SERVE_WINDOW};
    use secp256k1::{Keypair, Secp256k1};
    use std::collections::HashMap;

    fn create_state_provider_with_beacon_root_contract() -> StateProviderTest {
        let mut db = StateProviderTest::default();

        let beacon_root_contract_account = Account {
            balance: U256::ZERO,
            bytecode_hash: Some(keccak256(BEACON_ROOTS_CODE.clone())),
            nonce: 1,
        };

        db.insert_account(
            BEACON_ROOTS_ADDRESS,
            beacon_root_contract_account,
            Some(BEACON_ROOTS_CODE.clone()),
            HashMap::new(),
        );

        db
    }

    fn create_state_provider_with_withdrawal_requests_contract() -> StateProviderTest {
        let mut db = StateProviderTest::default();

        let withdrawal_requests_contract_account = Account {
            nonce: 1,
            balance: U256::ZERO,
            bytecode_hash: Some(keccak256(WITHDRAWAL_REQUEST_PREDEPLOY_CODE.clone())),
        };

        db.insert_account(
            WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
            withdrawal_requests_contract_account,
            Some(WITHDRAWAL_REQUEST_PREDEPLOY_CODE.clone()),
            HashMap::new(),
        );

        db
    }

    fn executor_provider(
        chain_spec: Arc<ChainSpec>,
    ) -> EthExecutorProvider<MockBitcoindFactory, Arc<TempDatabase<DatabaseEnv>>, EthEvmConfig>
    {
        create_noop_executor_provider(chain_spec)
    }

    #[test]
    fn eip_4788_non_genesis_call() {
        let mut header =
            Header { timestamp: 1, number: 1, excess_blob_gas: Some(0), ..Header::default() };
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);

        let db = create_state_provider_with_beacon_root_contract();

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(1))
                .build(),
        );

        let provider = executor_provider(chain_spec);

        // attempt to execute a block without parent beacon block root, expect err
        let err = provider
            .executor(StateProviderDatabase::new(&db))
            .execute(
                (
                    &BlockWithSenders {
                        block: Block {
                            header: header.clone(),
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect_err(
                "Executing cancun block without parent beacon block root field should fail",
            );

        assert_eq!(
            err.as_validation().unwrap().clone(),
            BlockValidationError::MissingParentBeaconBlockRoot
        );

        // fix header, set a gas limit
        header.parent_beacon_block_root = Some(B256::with_last_byte(0x69));

        let mut executor = provider.executor(StateProviderDatabase::new(&db));

        // Now execute a block with the fixed header, ensure that it does not fail
        executor
            .execute_without_verification(
                &BlockWithSenders {
                    block: Block {
                        header: header.clone(),
                        body: vec![],
                        ommers: vec![],
                        withdrawals: None,
                        requests: None,
                    },
                    senders: vec![],
                },
                U256::ZERO,
            )
            .unwrap();

        // check the actual storage of the contract - it should be:
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH should be
        // header.timestamp
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH + HISTORY_BUFFER_LENGTH
        //   // should be parent_beacon_block_root
        let history_buffer_length = 8191u64;
        let timestamp_index = header.timestamp % history_buffer_length;
        let parent_beacon_block_root_index =
            timestamp_index % history_buffer_length + history_buffer_length;

        // get timestamp storage and compare
        let timestamp_storage =
            executor.state.storage(BEACON_ROOTS_ADDRESS, U256::from(timestamp_index)).unwrap();
        assert_eq!(timestamp_storage, U256::from(header.timestamp));

        // get parent beacon block root storage and compare
        let parent_beacon_block_root_storage = executor
            .state
            .storage(BEACON_ROOTS_ADDRESS, U256::from(parent_beacon_block_root_index))
            .expect("storage value should exist");
        assert_eq!(parent_beacon_block_root_storage, U256::from(0x69));
    }

    #[test]
    fn eip_4788_no_code_cancun() {
        // This test ensures that we "silently fail" when cancun is active and there is no code at
        // // BEACON_ROOTS_ADDRESS
        let mut header = Header {
            timestamp: 1,
            number: 1,
            parent_beacon_block_root: Some(B256::with_last_byte(0x69)),
            excess_blob_gas: Some(0),
            ..Header::default()
        };
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);

        let db = StateProviderTest::default();

        // DON'T deploy the contract at genesis
        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(1))
                .build(),
        );

        let provider = executor_provider(chain_spec);

        // attempt to execute an empty block with parent beacon block root, this should not fail
        provider
            .batch_executor(StateProviderDatabase::new(&db))
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while cancun is active should not fail",
            );
    }

    #[test]
    fn eip_4788_empty_account_call() {
        // This test ensures that we do not increment the nonce of an empty SYSTEM_ADDRESS account
        // // during the pre-block call

        let mut db = create_state_provider_with_beacon_root_contract();

        // insert an empty SYSTEM_ADDRESS
        db.insert_account(SYSTEM_ADDRESS, Account::default(), None, HashMap::new());

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(1))
                .build(),
        );

        let provider = executor_provider(chain_spec);

        // construct the header for block one
        let mut header = Header {
            timestamp: 1,
            number: 1,
            parent_beacon_block_root: Some(B256::with_last_byte(0x69)),
            excess_blob_gas: Some(0),
            ..Header::default()
        };
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);

        let mut executor = provider.batch_executor(StateProviderDatabase::new(&db));

        // attempt to execute an empty block with parent beacon block root, this should not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while cancun is active should not fail",
            );

        // ensure that the nonce of the system address account has not changed
        let nonce = executor.state_mut().basic(SYSTEM_ADDRESS).unwrap().unwrap().nonce;
        assert_eq!(nonce, 0);
    }

    #[test]
    fn eip_4788_genesis_call() {
        let db = create_state_provider_with_beacon_root_contract();

        // activate cancun at genesis
        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(0))
                .build(),
        );

        let mut header = chain_spec.genesis_header();
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);
        let provider = executor_provider(chain_spec);
        let mut executor = provider.batch_executor(StateProviderDatabase::new(&db));

        // attempt to execute the genesis block with non-zero parent beacon block root, expect err
        header.parent_beacon_block_root = Some(B256::with_last_byte(0x69));
        let _err = executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header: header.clone(),
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect_err(
                "Executing genesis cancun block with non-zero parent beacon block root field
    should fail",
            );

        // fix header
        header.parent_beacon_block_root = Some(B256::ZERO);

        // now try to process the genesis block again, this time ensuring that a system contract
        // call does not occur
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .unwrap();

        // there is no system contract call so there should be NO STORAGE CHANGES
        // this means we'll check the transition state
        let transition_state = executor
            .state_mut()
            .transition_state
            .take()
            .expect("the evm should be initialized with bundle updates");

        // assert that it is the default (empty) transition state
        assert_eq!(transition_state, TransitionState::default());
    }

    #[test]
    fn eip_4788_high_base_fee() {
        // This test ensures that if we have a base fee, then we don't return an error when the
        // system contract is called, due to the gas price being less than the base fee.
        let mut header = Header {
            timestamp: 1,
            number: 1,
            parent_beacon_block_root: Some(B256::with_last_byte(0x69)),
            base_fee_per_gas: Some(u64::MAX),
            excess_blob_gas: Some(0),
            ..Header::default()
        };
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);

        let db = create_state_provider_with_beacon_root_contract();

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(1))
                .build(),
        );

        let provider = executor_provider(chain_spec);

        // execute header
        let mut executor = provider.batch_executor(StateProviderDatabase::new(&db));

        // Now execute a block with the fixed header, ensure that it does not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header: header.clone(),
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .unwrap();

        // check the actual storage of the contract - it should be:
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH should be
        // header.timestamp
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH + HISTORY_BUFFER_LENGTH
        //   // should be parent_beacon_block_root
        let history_buffer_length = 8191u64;
        let timestamp_index = header.timestamp % history_buffer_length;
        let parent_beacon_block_root_index =
            timestamp_index % history_buffer_length + history_buffer_length;

        // get timestamp storage and compare
        let timestamp_storage = executor
            .state_mut()
            .storage(BEACON_ROOTS_ADDRESS, U256::from(timestamp_index))
            .unwrap();
        assert_eq!(timestamp_storage, U256::from(header.timestamp));

        // get parent beacon block root storage and compare
        let parent_beacon_block_root_storage = executor
            .state_mut()
            .storage(BEACON_ROOTS_ADDRESS, U256::from(parent_beacon_block_root_index))
            .unwrap();
        assert_eq!(parent_beacon_block_root_storage, U256::from(0x69));
    }

    fn create_state_provider_with_block_hashes(latest_block: u64) -> StateProviderTest {
        let mut db = StateProviderTest::default();
        for block_number in 0..=latest_block {
            db.insert_block_hash(block_number, keccak256(block_number.to_string()));
        }
        db
    }

    #[test]
    fn eip_2935_pre_fork() {
        let db = create_state_provider_with_block_hashes(1);

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(EthereumHardfork::Prague, ForkCondition::Never)
                .build(),
        );

        let provider = executor_provider(chain_spec);
        let mut executor = provider.batch_executor(StateProviderDatabase::new(&db));

        // construct the header for block one
        let mut header = Header { timestamp: 1, number: 1, ..Header::default() };
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);

        // attempt to execute an empty block, this should not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // ensure that the block hash was *not* written to storage, since this is before the fork
        // was activated
        //
        // we load the account first, which should also not exist, because revm expects it to be
        // loaded
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_none());
        assert!(executor
            .state_mut()
            .storage(HISTORY_STORAGE_ADDRESS, U256::ZERO)
            .unwrap()
            .is_zero());
    }

    #[test]
    fn eip_2935_fork_activation_genesis() {
        let db = create_state_provider_with_block_hashes(0);

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(EthereumHardfork::Prague, ForkCondition::Timestamp(0))
                .build(),
        );

        let mut header = chain_spec.genesis_header();
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);
        let provider = executor_provider(chain_spec);
        let mut executor = provider.batch_executor(StateProviderDatabase::new(&db));

        // attempt to execute genesis block, this should not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // ensure that the block hash was *not* written to storage, since there are no blocks
        // preceding genesis
        //
        // we load the account first, which should also not exist, because revm expects it to be
        // loaded
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_none());
        assert!(executor
            .state_mut()
            .storage(HISTORY_STORAGE_ADDRESS, U256::ZERO)
            .unwrap()
            .is_zero());
    }

    #[test]
    fn eip_2935_fork_activation_within_window_bounds() {
        let fork_activation_block = (BLOCKHASH_SERVE_WINDOW - 10) as u64;
        let db = create_state_provider_with_block_hashes(fork_activation_block);

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(EthereumHardfork::Prague, ForkCondition::Timestamp(1))
                .build(),
        );

        let mut header = Header {
            parent_hash: B256::random(),
            timestamp: 1,
            number: fork_activation_block,
            requests_root: Some(EMPTY_ROOT_HASH),
            ..Header::default()
        };
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);
        let provider = executor_provider(chain_spec);
        let mut executor = provider.batch_executor(StateProviderDatabase::new(&db));

        // attempt to execute the fork activation block, this should not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // the hash for the ancestor of the fork activation block should be present
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_some());
        assert_ne!(
            executor
                .state_mut()
                .storage(HISTORY_STORAGE_ADDRESS, U256::from(fork_activation_block - 1))
                .unwrap(),
            U256::ZERO
        );

        // the hash of the block itself should not be in storage
        assert!(executor
            .state_mut()
            .storage(HISTORY_STORAGE_ADDRESS, U256::from(fork_activation_block))
            .unwrap()
            .is_zero());
    }

    #[test]
    fn eip_2935_fork_activation_outside_window_bounds() {
        let fork_activation_block = (BLOCKHASH_SERVE_WINDOW + 256) as u64;
        let db = create_state_provider_with_block_hashes(fork_activation_block);

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(EthereumHardfork::Prague, ForkCondition::Timestamp(1))
                .build(),
        );

        let provider = executor_provider(chain_spec);
        let mut executor = provider.batch_executor(StateProviderDatabase::new(&db));

        let mut header = Header {
            parent_hash: B256::random(),
            timestamp: 1,
            number: fork_activation_block,
            requests_root: Some(EMPTY_ROOT_HASH),
            ..Header::default()
        };
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);

        // attempt to execute the fork activation block, this should not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // the hash for the ancestor of the fork activation block should be present
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_some());
        assert_ne!(
            executor
                .state_mut()
                .storage(
                    HISTORY_STORAGE_ADDRESS,
                    U256::from(fork_activation_block % BLOCKHASH_SERVE_WINDOW as u64 - 1)
                )
                .unwrap(),
            U256::ZERO
        );
    }

    #[test]
    fn eip_2935_state_transition_inside_fork() {
        let db = create_state_provider_with_block_hashes(2);

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(EthereumHardfork::Prague, ForkCondition::Timestamp(0))
                .build(),
        );

        let mut header = chain_spec.genesis_header();
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);
        header.requests_root = Some(EMPTY_ROOT_HASH);
        let header_hash = header.hash_slow();

        let provider = executor_provider(chain_spec);
        let mut executor = provider.batch_executor(StateProviderDatabase::new(&db));

        // attempt to execute the genesis block, this should not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // nothing should be written as the genesis has no ancestors
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_none());
        assert!(executor
            .state_mut()
            .storage(HISTORY_STORAGE_ADDRESS, U256::ZERO)
            .unwrap()
            .is_zero());

        // attempt to execute block 1, this should not fail
        let mut header = Header {
            parent_hash: header_hash,
            timestamp: 1,
            number: 1,
            requests_root: Some(EMPTY_ROOT_HASH),
            ..Header::default()
        };
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);
        let header_hash = header.hash_slow();

        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // the block hash of genesis should now be in storage, but not block 1
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_some());
        assert_ne!(
            executor.state_mut().storage(HISTORY_STORAGE_ADDRESS, U256::ZERO).unwrap(),
            U256::ZERO
        );
        assert!(executor
            .state_mut()
            .storage(HISTORY_STORAGE_ADDRESS, U256::from(1))
            .unwrap()
            .is_zero());

        // attempt to execute block 2, this should not fail
        let mut header = Header {
            parent_hash: header_hash,
            timestamp: 1,
            number: 2,
            requests_root: Some(EMPTY_ROOT_HASH),
            ..Header::default()
        };
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);

        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // the block hash of genesis and block 1 should now be in storage, but not block 2
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_some());
        assert_ne!(
            executor.state_mut().storage(HISTORY_STORAGE_ADDRESS, U256::ZERO).unwrap(),
            U256::ZERO
        );
        assert_ne!(
            executor.state_mut().storage(HISTORY_STORAGE_ADDRESS, U256::from(1)).unwrap(),
            U256::ZERO
        );
        assert!(executor
            .state_mut()
            .storage(HISTORY_STORAGE_ADDRESS, U256::from(2))
            .unwrap()
            .is_zero());
    }

    #[test]
    fn eip_7002() {
        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(EthereumHardfork::Prague, ForkCondition::Timestamp(0))
                .build(),
        );

        let mut db = create_state_provider_with_withdrawal_requests_contract();

        let secp = Secp256k1::new();
        let sender_key_pair = Keypair::new(&secp, &mut generators::rng());
        let sender_address = public_key_to_address(sender_key_pair.public_key());

        db.insert_account(
            sender_address,
            Account { nonce: 1, balance: U256::from(ETH_TO_WEI), bytecode_hash: None },
            None,
            HashMap::new(),
        );

        // https://github.com/lightclient/7002asm/blob/e0d68e04d15f25057af7b6d180423d94b6b3bdb3/test/Contract.t.sol.in#L49-L64
        let validator_public_key = fixed_bytes!("111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111");
        let withdrawal_amount = fixed_bytes!("2222222222222222");
        let input: Bytes = [&validator_public_key[..], &withdrawal_amount[..]].concat().into();
        assert_eq!(input.len(), 56);

        let mut header = chain_spec.genesis_header();
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);
        header.gas_limit = 1_500_000;
        header.gas_used = 134_807;
        header.receipts_root =
            b256!("b31a3e47b902e9211c4d349af4e4c5604ce388471e79ca008907ae4616bb0ed3");

        let tx = sign_tx_with_key_pair(
            sender_key_pair,
            Transaction::Legacy(TxLegacy {
                chain_id: Some(chain_spec.chain.id()),
                nonce: 1,
                gas_price: header.base_fee_per_gas.unwrap().into(),
                gas_limit: 134_807,
                to: TxKind::Call(WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS),
                // `MIN_WITHDRAWAL_REQUEST_FEE`
                value: U256::from(1),
                input,
            }),
        );

        let provider = executor_provider(chain_spec);

        let executor = provider.executor(StateProviderDatabase::new(&db));

        let BlockExecutionOutput { receipts, requests, .. } = executor
            .execute(
                (
                    &Block {
                        header,
                        body: vec![tx],
                        ommers: vec![],
                        withdrawals: None,
                        requests: None,
                    }
                    .with_recovered_senders()
                    .unwrap(),
                    U256::ZERO,
                )
                    .into(),
            )
            .unwrap();

        let receipt = receipts.first().unwrap();
        assert!(receipt.success);

        let request = requests.first().unwrap();
        let withdrawal_request = request.as_withdrawal_request().unwrap();
        assert_eq!(withdrawal_request.source_address, sender_address);
        assert_eq!(withdrawal_request.validator_pubkey, validator_public_key);
        assert_eq!(withdrawal_request.amount, u64::from_be_bytes(withdrawal_amount.into()));
    }

    #[test]
    fn block_gas_limit_error() {
        // Create a chain specification with fork conditions set for Prague
        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(EthereumHardfork::Prague, ForkCondition::Timestamp(0))
                .build(),
        );

        // Create a state provider with the withdrawal requests contract pre-deployed
        let mut db = create_state_provider_with_withdrawal_requests_contract();

        // Initialize Secp256k1 for key pair generation
        let secp = Secp256k1::new();
        // Generate a new key pair for the sender
        let sender_key_pair = Keypair::new(&secp, &mut generators::rng());
        // Get the sender's address from the public key
        let sender_address = public_key_to_address(sender_key_pair.public_key());

        // Insert the sender account into the state with a nonce of 1 and a balance of 1 ETH in Wei
        db.insert_account(
            sender_address,
            Account { nonce: 1, balance: U256::from(ETH_TO_WEI), bytecode_hash: None },
            None,
            HashMap::new(),
        );

        // Define the validator public key and withdrawal amount as fixed bytes
        let validator_public_key = fixed_bytes!("111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111");
        let withdrawal_amount = fixed_bytes!("2222222222222222");
        // Concatenate the validator public key and withdrawal amount into a single byte array
        let input: Bytes = [&validator_public_key[..], &withdrawal_amount[..]].concat().into();
        // Ensure the input length is 56 bytes
        assert_eq!(input.len(), 56);

        // Create a genesis block header with a specified gas limit and gas used
        let mut header = chain_spec.genesis_header();
        header.gas_limit = 1_500_000;
        header.gas_used = 134_807;
        header.receipts_root =
            b256!("b31a3e47b902e9211c4d349af4e4c5604ce388471e79ca008907ae4616bb0ed3");

        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);

        // Create a transaction with a gas limit higher than the block gas limit
        let tx = sign_tx_with_key_pair(
            sender_key_pair,
            Transaction::Legacy(TxLegacy {
                chain_id: Some(chain_spec.chain.id()),
                nonce: 1,
                gas_price: header.base_fee_per_gas.unwrap().into(),
                gas_limit: 2_500_000, // higher than block gas limit
                to: TxKind::Call(WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS),
                value: U256::from(1),
                input,
            }),
        );

        // Create an executor from the state provider
        let executor = executor_provider(chain_spec).executor(StateProviderDatabase::new(&db));

        // Execute the block and capture the result
        let exec_result = executor.execute(
            (
                &Block {
                    header,
                    body: vec![tx],
                    ommers: vec![],
                    withdrawals: None,
                    requests: None,
                }
                .with_recovered_senders()
                .unwrap(),
                U256::ZERO,
            )
                .into(),
        );

        // Check if the execution result is an error and assert the specific error type
        match exec_result {
            Ok(_) => panic!("Expected block gas limit error"),
            Err(err) => assert_eq!(
                *err.as_validation().unwrap(),
                BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: 2_500_000,
                    block_available_gas: 1_500_000,
                }
            ),
        }
    }
}
