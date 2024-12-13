/// The purpose of this module is to provide a bridge between the CometBFT and the EVM
/// application state
use std::{
    error::Error,
    io::{self},
    sync::{Arc, RwLock},
};

use btcserverlib::extended_client::BtcServerExtendedClient;
use reth_basic_payload_builder::{BuildArguments, PayloadConfig};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_consensus::Consensus;
use reth_consensus_common::utils::unix_timestamp;
use reth_ethereum_payload_builder::default_ethereum_payload_builder;
use reth_evm::execute::BlockExecutorProvider;
use reth_network::NetworkHandle;
use reth_node_ethereum::EthEngineTypes;

use reth_blockchain_tree_api::{BlockValidationKind, BlockchainTreeEngine};
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_primitives::{
    botanix::block_with_peg::SealedBlockWithPeg, header_ext::HeaderExt, Address, BlockHash,
    SealedBlock, TransactionSigned,
};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use reth_revm::primitives::FixedBytes;
use reth_rpc_types::{engine::PayloadAttributes, BlockId};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::{EthPooledTransaction, EthTransactionValidator, TransactionPool};
use schnellru::{ByLength, LruMap};

use comet_bft_rpc::HttpCometBFTRpcClientFactory;

use tendermint_abci::{Application, ServerBuilder};
use tendermint_proto::{
    abci::{
        ExecTxResult, RequestPrepareProposal, RequestProcessProposal, ResponseCommit,
        ResponsePrepareProposal, ResponseProcessProposal,
    },
    v0_38::abci::{
        RequestCheckTx, RequestFinalizeBlock, RequestInfo, RequestInitChain, ResponseCheckTx,
        ResponseFinalizeBlock, ResponseInfo, ResponseInitChain,
    },
};

use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, error, info, warn};

use crate::{
    builder::BitcoinCheckpoint,
    comet_bft::{
        non_deterministic_data::NonDeterministicData, utils::transactions_signed_from_bytes,
    },
    engine_util::{
        self,
        SendForkChoiceUpdateError::{EngineError, InvalidPayload, RecvError},
    },
    excecution_utils::authority_execution_utils::build_and_execute,
    metrics::AuthorityMetrics,
    utils::{call_notify_pegin, call_notify_pegout},
    AuthorityConsensus, Storage,
};

/// Consts
const SUCCESS: u32 = 0;
const ERROR: u32 = 1;

// https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#verifystatus
const _VERIFY_UNKNOWN: i32 = 0;
const VERIFY_ACCEPTED: i32 = 1;
const VERIFY_REJECT: i32 = 2;

#[derive(Clone, Debug)]
pub struct ABCIClientBuilder<EF, BF, DB> {
    storage: Storage<EF, BF, DB>,
    bitcoin_checkpoint: BitcoinCheckpoint,
    network_handle: NetworkHandle,
    btc_server: Option<BtcServerExtendedClient>,
    authority_consensus: AuthorityConsensus,
    to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
    cbft_rpc_client_factory: HttpCometBFTRpcClientFactory,
    is_fed_node: bool,
    metrics: Arc<AuthorityMetrics>,
}

impl<EF, BF, DB> ABCIClientBuilder<EF, BF, DB>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    BF: BitcoindFactory + Clone + Unpin + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        storage: Storage<EF, BF, DB>,
        bitcoin_checkpoint: BitcoinCheckpoint,
        network_handle: NetworkHandle,
        btc_server: Option<BtcServerExtendedClient>,
        authority_consensus: AuthorityConsensus,
        to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
        cbft_rpc_client_factory: HttpCometBFTRpcClientFactory,
        is_fed_node: bool,
        metrics: Arc<AuthorityMetrics>,
    ) -> Self {
        Self {
            storage,
            bitcoin_checkpoint,
            network_handle,
            btc_server,
            authority_consensus,
            to_engine,
            cbft_rpc_client_factory,
            is_fed_node,
            metrics,
        }
    }

    pub async fn start_server<Pool: TransactionPool + Clone + 'static>(
        &self,
        task_executor: &impl TaskSpawner,
        validator: EthTransactionValidator<DB, EthPooledTransaction>,
        tx_pool: Pool,
        abci_host: String,
        abci_port: u16,
    ) -> Result<(), tendermint_abci::Error> {
        let (driver_tx, driver_rx) = tokio::sync::mpsc::channel(100);

        let app = ABCIClient::new(
            self.storage.clone(),
            validator,
            tx_pool,
            self.bitcoin_checkpoint.clone(),
            driver_tx,
            self.cbft_rpc_client_factory.clone(),
            self.authority_consensus.clone(),
            self.is_fed_node,
            self.metrics.clone(),
        );
        let mut abci_driver = ABCIDriver::new(
            self.storage.clone(),
            self.cbft_rpc_client_factory.clone(),
            self.authority_consensus.clone(),
            self.btc_server.clone(),
            self.network_handle.clone(),
            driver_rx,
            self.to_engine.clone(),
            self.is_fed_node,
        );

        let server_builder = ServerBuilder::default();
        // server will always bind to default address
        // CometBFT will always run on the same machine and container
        let server = server_builder.bind(format!("{abci_host}:{abci_port}"), app)?;

        if self.is_fed_node {
            loop {
                let storage = self.storage.inner.read().await;
                if storage.aggregate_public_key.is_some() {
                    info!(
                        "Aggregate public key is stored in the storage continuing to start ABCI server"
                    );
                    break;
                }
                info!(
                    "Waiting for aggregate public key to be stored in the storage before starting ABCI server"
                );
                drop(storage);
                tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
            }
        }

        task_executor.spawn_critical(
            "ABCI Client",
            Box::pin(async move {
                // we should panic here if cannot launch the ABCI server
                server.listen().expect("to start server");
            }),
        );
        task_executor.spawn_critical(
            "ABCI Driver",
            Box::pin(async move {
                if let Err(e) = abci_driver.start().await {
                    panic!("ABCI Driver encountered a error: {:?}", e);
                }
            }),
        );
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ABCIClient<EF, BF, DB, Pool> {
    storage: Storage<EF, BF, DB>,
    validator: EthTransactionValidator<DB, EthPooledTransaction>,
    pool: Pool,
    bitcoin_checkpoint: BitcoinCheckpoint,
    block_cache: Arc<RwLock<LruMap<BlockHash, SealedBlockWithPeg>>>,
    driver_tx: tokio::sync::mpsc::Sender<ABCIDriverMessage>,
    #[allow(dead_code)]
    cbft_rpc_provider: HttpCometBFTRpcClientFactory,
    authority_consensus: AuthorityConsensus,
    is_fed_node: bool,
    metrics: Arc<AuthorityMetrics>,
}

impl<EF, BF, DB, Pool> ABCIClient<EF, BF, DB, Pool>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    BF: BitcoindFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Clone + 'static,
{
    #[allow(clippy::too_many_arguments)]
    fn new(
        storage: Storage<EF, BF, DB>,
        validator: EthTransactionValidator<DB, EthPooledTransaction>,
        pool: Pool,
        bitcoin_checkpoint: BitcoinCheckpoint,
        driver_tx: tokio::sync::mpsc::Sender<ABCIDriverMessage>,
        cbft_rpc_provider: HttpCometBFTRpcClientFactory,
        authority_consensus: AuthorityConsensus,
        is_fed_node: bool,
        metrics: Arc<AuthorityMetrics>,
    ) -> Self {
        Self {
            storage,
            validator,
            pool,
            bitcoin_checkpoint,
            // Saving the last 5 blocks that were proposed
            block_cache: Arc::new(RwLock::new(LruMap::new(ByLength::new(5)))),
            driver_tx,
            cbft_rpc_provider,
            authority_consensus,
            is_fed_node,
            metrics,
        }
    }

    /// Returns the payload builder config
    /// this method will block and wait for the storage lock
    fn payload_builder_arguments(&self) -> PayloadConfig<EthPayloadBuilderAttributes> {
        let client = self.storage.client.clone();
        let chain_spec = self.storage.chain_spec.clone();

        let best_header =
            client.latest_header().expect("should have latest").expect("should have header");
        let best_block = BlockReaderIdExt::block_by_id(&client, BlockId::latest())
            .expect("have latest")
            .expect("have block")
            .seal(best_header.hash());

        let parent_block =
            BlockReaderIdExt::block_by_id(&client, BlockId::hash(best_header.parent_hash))
                .expect("have parent")
                .expect("have block")
                .seal(best_header.parent_hash);

        // let builder_config = EthPayloadBuilderAttributes::new(best_block.hash(), );
        let payload_attributes = PayloadAttributes {
            // Attributes here dont really matter
            // We just want to build a payload with the best txs
            timestamp: unix_timestamp(),
            prev_randao: FixedBytes::<32>::random(),
            suggested_fee_recipient: Address::ZERO,
            withdrawals: None,
            parent_beacon_block_root: parent_block.parent_beacon_block_root,
        };

        let payload_builder_attributes =
            EthPayloadBuilderAttributes::new(best_block.hash(), payload_attributes);

        PayloadConfig::new(
            Arc::new(best_block),
            reth_primitives::Bytes::default(),
            payload_builder_attributes,
            chain_spec,
        )
    }

    pub(crate) fn non_deterministic_data_bytes(&self) -> prost::bytes::Bytes {
        let ndd = NonDeterministicData::new(self.bitcoin_blockhash(), self.aggregate_public_key());
        let ndd_bytes = prost::bytes::Bytes::copy_from_slice(
            ndd.serialize().expect("non deterministic data to be serialized").as_slice(),
        );

        ndd_bytes
    }

    pub(crate) fn validate_block(&self, block: &SealedBlock) -> ResponseProcessProposal {
        // validate_block_post_execution() is called when inserting the block (ABCIDriver)
        match self.authority_consensus.validate_block_pre_execution(block) {
            Ok(_) => {}
            Err(e) => {
                error!("Error in validate_block_pre_execution(): {:?}", e);
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        }

        // standard header validation
        match self.authority_consensus.validate_header(&block.header) {
            Ok(_) => {}
            Err(e) => {
                error!("Error in validate_header(): {:?}", e);
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        }

        // poa validation
        let agg_pk =
            self.storage.inner.blocking_read().aggregate_public_key.expect("agg pk to exist");
        match self.authority_consensus.validate_header_standalone(
            block.header(),
            self.storage.genesis_authorities.as_slice(),
            Some(&agg_pk),
        ) {
            Ok(_) => {}
            Err(e) => {
                error!("Error in validate_header_standalone(): {:?}", e);
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        }

        ResponseProcessProposal { status: VERIFY_ACCEPTED }
    }

    pub(crate) fn aggregate_public_key(&self) -> secp256k1::PublicKey {
        // TODO should return error if agg pk is not set
        self.storage.inner.blocking_read().aggregate_public_key.expect("agg pk exists")
    }

    pub(crate) fn bitcoin_blockhash(&self) -> bitcoin::BlockHash {
        self.bitcoin_checkpoint.blocking_read().expect("should have checkpoint").0.block_hash()
    }

    pub(crate) fn application_hash(&self, db: &impl BlockReaderIdExt) -> prost::bytes::Bytes {
        let header = db.latest_header().expect("should have latest").expect("should have header");
        prost::bytes::Bytes::copy_from_slice(&header.hash().0)
    }
}

impl<EF, BF, DB, Pool> Application for ABCIClient<EF, BF, DB, Pool>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    BF: BitcoindFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Clone + 'static,
{
    // docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#init_chain
    fn init_chain(&self, _request: RequestInitChain) -> ResponseInitChain {
        info!("init_chain request: {:?}", _request);
        let client = self.storage.client.clone();
        ResponseInitChain { app_hash: self.application_hash(&client), ..Default::default() }
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#info
    fn info(&self, request: RequestInfo) -> ResponseInfo {
        info!("info request: {:?}", request);
        let client = self.storage.client.clone();
        let latest_header =
            client.latest_header().expect("should have latest").expect("should have header");

        ResponseInfo {
            data: String::default(),
            version: "TODO".to_string(),
            app_version: 1,
            last_block_height: latest_header.number as i64,
            last_block_app_hash: self.application_hash(&client),
        }
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#prepareProposal
    fn prepare_proposal(&self, request: RequestPrepareProposal) -> ResponsePrepareProposal {
        info!("prepare_proposal request for height: {:?}", request.height);
        debug!("prepare_proposal request: {:?}", request);
        let _txs_bytes = request.txs;

        // insert non-deterministic data tx at index 0 so historical sync will pass verification
        let non_deterministic_data_bytes = self.non_deterministic_data_bytes();
        if self.pool.pool_size().total == 0 {
            info!("No transactions in pool, waiting...");

            return ResponsePrepareProposal { txs: vec![non_deterministic_data_bytes] };
        }

        let payload_config = self.payload_builder_arguments();
        let client = self.storage.client.clone();

        let build_args = BuildArguments {
            client,
            pool: self.pool.clone(),
            cached_reads: Default::default(),
            config: payload_config,
            cancel: Default::default(),
            best_payload: None,
        };
        let res = default_ethereum_payload_builder(self.storage.evm_config, build_args);
        let response = match res {
            Ok(res) => {
                match res {
                    reth_basic_payload_builder::BuildOutcome::Aborted {
                        fees: _,
                        cached_reads: _,
                    } => ResponsePrepareProposal { ..Default::default() },
                    reth_basic_payload_builder::BuildOutcome::Cancelled => {
                        ResponsePrepareProposal { ..Default::default() }
                    }
                    reth_basic_payload_builder::BuildOutcome::Better {
                        payload,
                        cached_reads: _,
                    } => {
                        let block = payload.block();
                        // These are bytes of [SignedTransaction]
                        let mut txs: Vec<_> = block
                            .raw_transactions()
                            .iter()
                            .map(|tx| prost::bytes::Bytes::copy_from_slice(tx))
                            .collect::<_>();
                        info!("prepare_proposal number of txs: {:?}", txs.len());

                        // insert non-deterministic data tx at index 0 so historical sync will pass
                        // verification
                        txs.insert(0, non_deterministic_data_bytes);
                        self.metrics.commet_prepared_proposals.increment(1);
                        ResponsePrepareProposal { txs }
                    }
                }
            }
            Err(e) => {
                error!("Error building payload: {:?}", e);
                ResponsePrepareProposal { ..Default::default() }
            }
        };

        response
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#checktx
    fn check_tx(&self, request: RequestCheckTx) -> ResponseCheckTx {
        info!("check_tx request: {:?}", request);
        // We are ignore type for now
        // One of CheckTx_New or CheckTx_Recheck. CheckTx_New is the default and means that a full
        // check of the tranasaction is required. CheckTx_Recheck types are used when the mempool is
        // initiating a normal recheck of a transaction.
        let _type = request.r#type;
        let _tx_bytes = request.tx.clone();
        let hex = match hex::decode(request.tx.clone()) {
            Ok(hex) => hex, // Proceed with the decoded hex if successful
            Err(err) => {
                return ResponseCheckTx {
                    code: 1,
                    log: format!("Failed to decode transaction: {}", err),
                    ..Default::default()
                };
            }
        };

        let mut error = (SUCCESS, "Ok");
        match TransactionSigned::decode_enveloped(&mut hex.as_slice()) {
            Ok(tx) => {
                if let Ok(tx_ec_recover) = tx.try_into_ecrecovered() {
                    let length = tx_ec_recover.length_without_header();
                    let pool_tx = EthPooledTransaction::new(tx_ec_recover, length);

                    let res = self.validator.validate_one(
                        reth_transaction_pool::TransactionOrigin::External,
                        pool_tx.clone(),
                    );

                    match res {
                        reth_transaction_pool::TransactionValidationOutcome::Valid {
                            balance: _,
                            state_nonce: _,
                            transaction: _,
                            propagate: _,
                        } => {} // Do nothing
                        reth_transaction_pool::TransactionValidationOutcome::Invalid(_, e) => {
                            error!("Txinvalid: Error validating transaction: {:?}", e);
                            error = (ERROR, "Error occurred while validating transaction");
                        }
                        reth_transaction_pool::TransactionValidationOutcome::Error(_, e) => {
                            error!("TxError: Error validating transaction: {:?}", e);
                            error = (ERROR, "Error occurred while validating transaction");
                        }
                    }
                } else {
                    error = (ERROR, "Error recovering tx signer. Invalid signature");
                }
            }
            Err(e) => {
                error!("Error decoding transaction: {:?}", e);
                error = (ERROR, "Error decoding transaction");
            }
        }

        self.metrics.commet_checked_txs.increment(1);
        ResponseCheckTx {
            code: error.0,
            log: error.1.to_string(),
            info: error.1.to_string(),
            ..Default::default()
        }
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#prepareproposal
    fn process_proposal(&self, request: RequestProcessProposal) -> ResponseProcessProposal {
        info!("process_proposal request for height: {:?}", request.height);
        debug!("process_proposal request: {:?}", request);
        let storage = self.storage.inner.blocking_read();
        let agg_pk = storage.aggregate_public_key;

        if agg_pk.is_none() {
            // Fed nodes must always have an aggregate public key
            if self.is_fed_node {
                warn!("Aggregate public key for fed node is not set in process proposal");
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }

            // Rpc nodes will have an aggregate public key above block height 1
            if request.height > 1 {
                warn!("Aggregate public key for rpc node is not set in process proposal");
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        }

        // Drop the lock
        drop(storage);

        // Extract who built this block
        let block_builder_address = Address::new(
            FixedBytes::<20>::from_slice(request.proposer_address.to_vec().as_slice()).0,
        );

        // Extract block time: this must come from the CBFT block header NOT the system time
        // As that will be underministic
        let block_time = request.time.expect("block time");
        let cbft_block_hash = FixedBytes::<32>::from_slice(request.hash.to_vec().as_slice());

        // extract first tx which contains non-deterministic data and validate
        let txs_bytes = request.txs;
        let non_deterministic_data_bytes = match txs_bytes.first() {
            Some(tx) => tx.clone(),
            None => {
                warn!("No non-deterministic tx in proposal request");
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        };
        let reader_inner: Vec<u8> =
            vec![non_deterministic_data_bytes].into_iter().flatten().collect();
        let mut reader = &mut io::Cursor::new(reader_inner);
        let non_deterministic_data = match NonDeterministicData::deserialize(&mut reader) {
            Ok(data) => data,
            Err(e) => {
                warn!("Error deserializing non-deterministic data: {:?}", e);
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        };

        let bitcoin_checkpoint_block_hash =
            self.bitcoin_checkpoint.blocking_read().expect("should have checkpoint").0.block_hash();

        // check non-deterministic data: btc block hash and aggregate public key
        if bitcoin_checkpoint_block_hash != non_deterministic_data.bitcoin_block_hash {
            warn!("Bitcoin block hash mismatch");
            return ResponseProcessProposal { status: VERIFY_REJECT };
        }

        if agg_pk.expect("agg pk to be defined") != non_deterministic_data.aggregated_public_key {
            warn!("Aggregate public key mismatch");
            return ResponseProcessProposal { status: VERIFY_REJECT };
        }

        // get txs skipping the first non-deterministic data tx
        let txs = match transactions_signed_from_bytes(txs_bytes.iter().skip(1).cloned()) {
            Ok(txs) => txs,
            Err(e) => {
                error!("Error decoding transactions: {:?}", e);
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        };

        match build_and_execute(
            txs,
            self.storage.chain_spec.clone(),
            &block_builder_address,
            self.storage.evm_config,
            &self.storage.client,
            &self.storage.bitcoind_factory,
            self.storage.btc_network,
            &bitcoin_checkpoint_block_hash,
            &agg_pk.expect("agg pk to be defined"),
            &self.storage.genesis_authorities,
            block_time,
        ) {
            Ok((exec_results, block)) => {
                info!("Block built successfully, resulting block hash: {:?}", block.hash_slow());
                let block_hash = block.hash_slow();
                info!("Block built successfully, resulting block hash: {:?}", block_hash);
                let sealed_block_with_sender =
                    block.seal_slow().try_seal_with_senders().expect("to seal");
                let sealed_block_with_peg = SealedBlockWithPeg::new(
                    sealed_block_with_sender,
                    exec_results.pegins,
                    exec_results.pegouts,
                );

                // validate block before caching
                match self.validate_block(&sealed_block_with_peg.block().block.clone()) {
                    ResponseProcessProposal { status: VERIFY_ACCEPTED } => {}
                    _ => {
                        return ResponseProcessProposal { status: VERIFY_REJECT };
                    }
                }

                self.block_cache
                    .write()
                    .expect("to get write lock")
                    .insert(cbft_block_hash, sealed_block_with_peg);
            }
            Err(e) => {
                error!("Error building block: {:?}", e);
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        }
        self.metrics.commet_processed_proposals.increment(1);
        ResponseProcessProposal { status: VERIFY_ACCEPTED }
    }

    ///docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#finalizeblock
    fn finalize_block(&self, request: RequestFinalizeBlock) -> ResponseFinalizeBlock {
        info!(
            "finalize_block request for height: {:?}, number of txs: {:?}",
            request.height,
            request.txs.len()
        );
        debug!("finalize_block request: {:?}", request);
        let cbft_block_hash = FixedBytes::<32>::from_slice(request.hash.to_vec().as_slice());
        let mut block_cache_write = self.block_cache.write().expect("should get write lock");
        let sealed_block_with_peg = match block_cache_write.get(&cbft_block_hash) {
            Some(block) => block.clone(),
            None => {
                // No block in cache: this happens during historical (block) sync
                // Build the block

                // get non-deterministic data
                let txs_bytes = request.txs.clone();
                let non_deterministic_data_bytes = match txs_bytes.clone().first() {
                    Some(tx) => tx.clone(),
                    None => panic!("No non-deterministic tx in finalize block request"),
                };
                let reader_inner: Vec<u8> =
                    vec![non_deterministic_data_bytes].into_iter().flatten().collect();
                let mut reader = &mut io::Cursor::new(reader_inner);
                let non_deterministic_data = match NonDeterministicData::deserialize(&mut reader) {
                    Ok(data) => data,
                    Err(e) => panic!("Error deserializing non-deterministic data: {:?}", e),
                };

                let block_time = request.time.expect("block time");

                // Extract who built this block
                let block_builder_address = Address::new(
                    FixedBytes::<20>::from_slice(request.proposer_address.to_vec().as_slice()).0,
                );

                // get txs skipping the first non-deterministic data tx
                let txs =
                    match transactions_signed_from_bytes(txs_bytes.clone().iter().skip(1).cloned())
                    {
                        Ok(txs) => txs,
                        Err(e) => panic!("Error decoding transactions in finalize block: {:?}", e),
                    };

                match build_and_execute(
                    txs,
                    self.storage.chain_spec.clone(),
                    &block_builder_address,
                    self.storage.evm_config,
                    &self.storage.client,
                    &self.storage.bitcoind_factory,
                    self.storage.btc_network,
                    &non_deterministic_data.bitcoin_block_hash,
                    &non_deterministic_data.aggregated_public_key,
                    &self.storage.genesis_authorities,
                    block_time,
                ) {
                    Ok((exec_results, block)) => {
                        let block_hash = block.hash_slow();
                        info!("Block built successfully, resulting block hash: {:?}", block_hash);
                        let sealed_block_with_sender =
                            block.seal_slow().try_seal_with_senders().expect("to seal");
                        let sealed_block_with_peg = SealedBlockWithPeg::new(
                            sealed_block_with_sender,
                            exec_results.pegins,
                            exec_results.pegouts,
                        );

                        block_cache_write.insert(cbft_block_hash, sealed_block_with_peg.clone());

                        sealed_block_with_peg
                    }
                    Err(e) => panic!("Error building block in finalize block: {:?}", e),
                }
            }
        };

        let mut exec_results = vec![];
        // insert non-deterministic data tx which is first in the block
        let non_deterministic_data_tx = request.txs.first().expect("tx to exist").clone();
        let first_exec_tx_result =
            ExecTxResult { code: SUCCESS, data: non_deterministic_data_tx, ..Default::default() };
        exec_results.push(first_exec_tx_result);

        for _tx in sealed_block_with_peg.block().body.iter() {
            // https://docs.cometbft.com/v0.38/spec/abci/abci++_app_requirements#transaction-results
            exec_results.push(ExecTxResult {
                code: SUCCESS,
                // From https://docs.cometbft.com/v0.38/spec/abci/abci++_app_requirements#gas
                // In v0.34.x and earlier versions, CometBFT does not enforce anything about Gas in
                // consensus, only in the mempool. ... The GasUsed field is ignored
                // by CometBFT. CometBFT's genesis.json should have max_gas set to
                // -1 as to not enforce any gas limit restrictions Gas and other
                // block resource limits are enforced by the EVM/Reth
                ..Default::default()
            });
        }

        let block_hash = sealed_block_with_peg.block().hash();
        self.metrics.commet_finalzied_blocks.increment(1);
        ResponseFinalizeBlock {
            events: vec![],
            tx_results: exec_results,
            validator_updates: vec![],
            consensus_param_updates: None,
            app_hash: prost::bytes::Bytes::copy_from_slice(&block_hash.0),
        }
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#commit
    fn commit(&self) -> ResponseCommit {
        info!("commit request received");
        let candidate_blocks = self.block_cache.write().expect("to get write lock");
        // We want to explicitly panic since we cannot get the lock and send the finalize message
        let (cbft_block_hash, sealed_block_with_peg) =
            candidate_blocks.peek_newest().expect("to have block");

        // We want to explicitly panic if we cannot send the finalize message
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        self.driver_tx
            .blocking_send(ABCIDriverMessage::CommitBlock((
                sealed_block_with_peg.clone(),
                *cbft_block_hash,
                tx,
            )))
            .expect("to send");

        rx.blocking_recv().expect("to receive");
        info!("Block finalized: {:?}", cbft_block_hash);
        self.metrics.commet_committed_blocks.increment(1);

        ResponseCommit::default()
    }
}

#[allow(dead_code)]
enum ABCIDriverMessage {
    /// Finalize a block, message includes the sealed block and the CBFT block hash
    CommitBlock((SealedBlockWithPeg, FixedBytes<32>, tokio::sync::oneshot::Sender<()>)),
    Exit,
}

// The driver is mainly responsible for driving block completion and finalization
// Once a finalize block is received the drive is responsible for
// * Updating the canonical chain via DB
// * Sending the finalized block to the network
// * Sending the finalized block to the engine (FCU)
// * Sending pegins / pegouts to the btc server
// * Updating the [ExtraDataHeader] with the block witnesses
#[allow(dead_code)]
pub(crate) struct ABCIDriver<EF, BF, DB> {
    storage: Storage<EF, BF, DB>,
    cbft_rpc_provider: HttpCometBFTRpcClientFactory,
    authority_consensus: AuthorityConsensus,
    btc_server: Option<BtcServerExtendedClient>,
    network_handle: NetworkHandle,
    driver_rx: tokio::sync::mpsc::Receiver<ABCIDriverMessage>,
    to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
    is_fed_node: bool,
}

impl<EF, BF, DB> ABCIDriver<EF, BF, DB>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    BF: BitcoindFactory + Clone + Unpin + 'static,
{
    #[allow(clippy::too_many_arguments)]
    fn new(
        storage: Storage<EF, BF, DB>,
        cbft_rpc_provider: HttpCometBFTRpcClientFactory,
        authority_consensus: AuthorityConsensus,
        btc_server: Option<BtcServerExtendedClient>,
        network_handle: NetworkHandle,
        driver_rx: tokio::sync::mpsc::Receiver<ABCIDriverMessage>,
        to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
        is_fed_node: bool,
    ) -> Self {
        Self {
            storage,
            cbft_rpc_provider,
            authority_consensus,
            btc_server,
            network_handle,
            driver_rx,
            to_engine,
            is_fed_node,
        }
    }

    async fn start(&mut self) -> Result<(), Box<dyn Error>> {
        loop {
            if let Some(message) = self.driver_rx.recv().await {
                match message {
                    ABCIDriverMessage::CommitBlock((sealed_block_with_peg, _cbft_hash, tx)) => {
                        let client = self.storage.client.clone();
                        let sealed_block_with_senders = sealed_block_with_peg.block();
                        let sealed_header = sealed_block_with_senders.header.clone();
                        let block_hash = sealed_header.hash();

                        // Update canonical chain
                        match client.insert_block(
                            sealed_block_with_senders.clone(),
                            BlockValidationKind::Exhaustive,
                        ) {
                            Ok(_) => {}
                            Err(e) => {
                                error!(target: "consensus::authority", ?e, "Failed to insert block");
                                return Err(Box::new(e));
                            }
                        }

                        // Rpc node needs to store aggregate public key from block height 1
                        if !self.is_fed_node && sealed_block_with_senders.number == 1 {
                            let edh = sealed_block_with_senders
                                .deserialize_extra_data_header()
                                .expect("edh to exist");

                            let mut storage = self.storage.inner.write().await;
                            storage.aggregate_public_key = Some(edh.aggregated_public_key);
                        }

                        client.set_canonical_head(sealed_block_with_senders.header.clone());
                        client.set_safe(sealed_block_with_senders.header.clone());
                        client.set_finalized(sealed_block_with_senders.header.clone());

                        let _engine = match engine_util::send_fork_choice_update_payload(
                            block_hash,
                            self.to_engine.clone(),
                        )
                        .await
                        {
                            Ok(_) => Ok(()),
                            Err(err) => match err {
                                EngineError => Err(EngineError),
                                InvalidPayload => Err(InvalidPayload),
                                _ => Err(RecvError),
                            },
                        };

                        // Annount to the network
                        let block_to_commit = sealed_block_with_senders.block.clone().unseal();
                        let block_height = block_to_commit.number;

                        let pegins = sealed_block_with_peg
                            .pegins()
                            .iter()
                            .flat_map(|p| p.meta.clone())
                            .collect::<Vec<_>>();

                        let pegouts = sealed_block_with_peg.pegouts();

                        // TODO what happens if the pegins fail? Should we panic? Should this be
                        // called in commit?
                        if self.btc_server.is_some() {
                            // pegins
                            call_notify_pegin(
                                self.btc_server.as_mut().expect("btc server to exist"),
                                &pegins,
                            )
                            .await
                            .expect("Should notify pegins");

                            // pegouts
                            call_notify_pegout(
                                self.btc_server.as_mut().expect("btc server to exist"),
                                pegouts,
                                block_height,
                            )
                            .await
                            .expect("Should notify pegouts");
                        }

                        tx.send(()).expect("to send");
                    }
                    ABCIDriverMessage::Exit => {
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{builder::BitcoinCheckpoint, Storage};
    use bitcoin::{
        block::{BlockHash, Header, Version},
        hashes::Hash,
        CompactTarget, TxMerkleNode,
    };
    use comet_bft_rpc::HttpCometBFTRpcClientFactory;
    use rand::thread_rng;
    use reth_blockchain_tree::noop::NoopBlockchainTree;
    use reth_btc_wallet::{
        bitcoind::{BitcoindConfig, BitcoindFactory},
        test_utils::MockBitcoindFactory,
    };
    use reth_chainspec::BOTANIX_TESTNET;
    use reth_cli_runner::tokio_runtime;
    use reth_db::{init_db, mdbx::DatabaseArguments};
    use reth_db_common::init::init_genesis;
    use reth_evm::test_utils::MockExecutorProvider;
    use reth_node_core::{args::TxPoolArgs, cli::config::RethTransactionPoolConfig};
    use reth_node_ethereum::EthEvmConfig;
    use reth_provider::providers::{BlockchainProvider, ProviderFactory, StaticFileProvider};
    use reth_revm::primitives::EnvKzgSettings;
    use reth_tasks::TaskManager;
    use reth_transaction_pool::{
        blobstore::InMemoryBlobStore, test_utils::TransactionGenerator, Pool as RethPool,
        TransactionOrigin, TransactionPool, TransactionValidationTaskExecutor,
    };
    use std::path::Path;
    use tempfile::tempdir;
    use tendermint_abci::Application;
    use tendermint_proto::google::protobuf::Timestamp;
    use tokio::sync::RwLock;

    /// Build the db and the ABCI client
    fn abci_client_builder() -> ABCIClient<
        MockExecutorProvider,
        MockBitcoindFactory,
        BlockchainProvider<Arc<reth_db::DatabaseEnv>>,
        RethPool<
            TransactionValidationTaskExecutor<
                EthTransactionValidator<
                    BlockchainProvider<Arc<reth_db::DatabaseEnv>>,
                    EthPooledTransaction,
                >,
            >,
            reth_transaction_pool::CoinbaseTipOrdering<EthPooledTransaction>,
            InMemoryBlobStore,
        >,
    > {
        let secp = secp256k1::Secp256k1::new();
        let sk = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let pk = secp256k1::PublicKey::from_secret_key(&secp, &sk);

        // setup db client
        let temp_dir = tempdir().expect("to create temp dir");
        let db_path = temp_dir.path().join("db");
        let db_path = Path::new(&db_path);
        let db = Arc::new(init_db(db_path, DatabaseArguments::default()).expect("to init db"));
        let spec = Arc::new(BOTANIX_TESTNET.as_ref().to_owned());
        let factory = ProviderFactory::new(
            db.clone(),
            spec.clone(),
            StaticFileProvider::read_write(db_path.join("static_files"))
                .expect("static file providerto exist"),
        );
        let _ = init_genesis(factory.clone()).expect("to init genesis");
        let client = BlockchainProvider::new(factory, Arc::new(NoopBlockchainTree::default()))
            .expect("to create blockchain provider");

        let storage = Storage::new(
            Vec::new(),
            0,
            pk,
            bitcoin::Network::Regtest,
            Some(pk),
            Vec::new(),
            EthEvmConfig::default(),
            BOTANIX_TESTNET.clone(),
            MockBitcoindFactory::new(BitcoindConfig::default()),
            MockExecutorProvider::default(),
            client.clone(),
        );

        // setup validator with task executor
        let blob_store = InMemoryBlobStore::default();
        let tokio_runtime: tokio::runtime::Runtime = tokio_runtime().expect("to create runtime");
        let task_manager = TaskManager::new(tokio_runtime.handle().clone());
        let task_executor = task_manager.executor();
        let validator = TransactionValidationTaskExecutor::eth_builder(storage.chain_spec.clone())
            .with_head_timestamp(0)
            .kzg_settings(EnvKzgSettings::Default)
            .with_additional_tasks(1)
            .build_with_tasks(client, task_executor, blob_store.clone());

        let transaction_pool =
            RethPool::eth_pool(validator.clone(), blob_store, TxPoolArgs::default().pool_config());

        let bitcoin_header = Header {
            version: Version::default(),
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: TxMerkleNode::from_slice(&[0; 32])
                .expect("Failed to create merkle root from slice"),
            time: 0,
            bits: CompactTarget::from_consensus(0),
            nonce: 0,
        };
        let bitcoin_checkpoint: BitcoinCheckpoint =
            Arc::new(RwLock::new(Some((bitcoin_header, 0))));

        let cometbft_rpc_factory = HttpCometBFTRpcClientFactory::default();

        let (driver_tx, _driver_rx) = tokio::sync::mpsc::channel(100);

        let abci_client = ABCIClient::new(
            storage,
            validator.validator,
            transaction_pool,
            bitcoin_checkpoint,
            driver_tx,
            cometbft_rpc_factory,
            AuthorityConsensus::new(spec),
            false,
            Arc::new(AuthorityMetrics::default()),
        );

        abci_client
    }

    #[test]
    fn test_init_chain() {
        let abci_client = abci_client_builder();

        let request = RequestInitChain::default();
        let response = abci_client.init_chain(request);

        let expected_consensus_params = None;
        let expected_validators = vec![];

        assert_eq!(response.consensus_params, expected_consensus_params);
        assert_eq!(response.validators, expected_validators);
        let _response_app_hash_hex = hex::encode(response.app_hash.to_vec().as_slice());
        assert_eq!(
            response.app_hash.to_vec(),
            BOTANIX_TESTNET.genesis_hash.expect("Failed to unwrap genesis hash").0.to_vec()
        );
    }

    #[test]
    fn test_info() {
        let abci_client = abci_client_builder();

        let request = RequestInfo::default();
        let response = abci_client.info(request);

        assert_eq!(response.data, String::default());
        assert_eq!(response.version, "TODO".to_string());
        assert_eq!(response.app_version, 1);
        assert_eq!(response.last_block_height, 0);
        let _response_app_hash_hex = hex::encode(response.last_block_app_hash.to_vec().as_slice());
        assert_eq!(
            response.last_block_app_hash.to_vec(),
            BOTANIX_TESTNET.genesis_hash.expect("Failed to unwrap genesis hash").0.to_vec()
        );
    }

    #[test]
    fn test_prepare_proposal_empty_mempool() {
        let abci_client = abci_client_builder();

        let request = RequestPrepareProposal::default();
        let response = abci_client.prepare_proposal(request);

        let expected_ndd = NonDeterministicData::new(
            abci_client.bitcoin_blockhash(),
            abci_client.aggregate_public_key(),
        );
        let response_ndd_bytes = response.txs.first().expect("to have tx").clone();
        let reader_inner: Vec<u8> = vec![response_ndd_bytes].into_iter().flatten().collect();
        let mut reader = &mut io::Cursor::new(reader_inner);
        let response_ndd = NonDeterministicData::deserialize(&mut reader).expect("to deserialize");

        assert_eq!(response.txs.len(), 1);
        assert_eq!(response_ndd, expected_ndd);
    }

    // TODO: fix error ValidationServiceUnreachable when adding tx to mempool
    #[test]
    #[ignore]
    fn test_prepare_proposal_tx_in_mempool() {
        let abci_client = abci_client_builder();

        let mut tx_generator = TransactionGenerator::new(thread_rng());
        let pooled_tx = tx_generator.gen_eip1559_pooled();

        let rt = tokio::runtime::Runtime::new().expect("to create runtime");

        rt.block_on(async {
            match abci_client.pool.add_transaction(TransactionOrigin::Local, pooled_tx).await {
                Ok(_) => {}
                Err(e) => {
                    panic!("Error adding tx to pool: {:?}", e);
                }
            }
        });

        let request = RequestPrepareProposal::default();
        let response = abci_client.prepare_proposal(request);

        let expected_ndd = NonDeterministicData::new(
            abci_client.bitcoin_blockhash(),
            abci_client.aggregate_public_key(),
        );
        let response_ndd_bytes = response.txs.first().expect("to have tx").clone();
        let reader_inner: Vec<u8> = vec![response_ndd_bytes].into_iter().flatten().collect();
        let mut reader = &mut io::Cursor::new(reader_inner);
        let response_ndd = NonDeterministicData::deserialize(&mut reader).expect("to deserialize");

        // todo: deserialize tx

        assert_eq!(response.txs.len(), 2);
        assert_eq!(response_ndd, expected_ndd);
    }

    #[test]
    fn test_process_proposal_with_ndd_tx_only() {
        let abci_client = abci_client_builder();

        let mut request = RequestProcessProposal::default();

        let ndd_bytes = abci_client.non_deterministic_data_bytes();

        request.txs = vec![ndd_bytes];

        let proposer_address = prost::bytes::Bytes::copy_from_slice(Address::ZERO.0.as_slice());
        request.proposer_address = proposer_address;

        request.time = Some(Timestamp::default());
        request.hash = prost::bytes::Bytes::copy_from_slice(FixedBytes::<32>::random().as_slice());

        let response = abci_client.process_proposal(request);

        assert_eq!(response.status, VERIFY_ACCEPTED);
    }

    #[test]
    fn test_process_proposal_with_signed_tx() {
        let abci_client = abci_client_builder();

        // first tx should be non-deterministic data
        let ndd_bytes = abci_client.non_deterministic_data_bytes();

        // second tx should be a signed transaction
        let mut tx_generator = TransactionGenerator::new(thread_rng());
        let signed_tx = tx_generator.transaction().into_legacy();
        let mut buf = Vec::new();
        signed_tx.encode_enveloped(&mut buf);
        let signed_tx_bytes = prost::bytes::Bytes::copy_from_slice(buf.as_slice());

        let mut request = RequestProcessProposal::default();
        request.txs = vec![ndd_bytes, signed_tx_bytes];

        let proposer_address = prost::bytes::Bytes::copy_from_slice(Address::ZERO.0.as_slice());
        request.proposer_address = proposer_address;

        request.time = Some(Timestamp::default());
        request.hash = prost::bytes::Bytes::copy_from_slice(FixedBytes::<32>::random().as_slice());

        let response = abci_client.process_proposal(request);

        // this fails bc prevrandao isn't being set in the evm env during tests
        // but all the custom code is executed successfully up to `build_and_execute`
        assert_eq!(response.status, VERIFY_REJECT);
    }

    #[test]
    fn test_finalize_block_with_ndd_tx_only() {
        let abci_client = abci_client_builder();

        let mut request = RequestFinalizeBlock::default();

        let ndd_bytes = abci_client.non_deterministic_data_bytes();

        request.txs = vec![ndd_bytes.clone()];

        let proposer_address = prost::bytes::Bytes::copy_from_slice(Address::ZERO.0.as_slice());
        request.proposer_address = proposer_address;

        request.time = Some(Timestamp::default());
        request.hash = prost::bytes::Bytes::copy_from_slice(FixedBytes::<32>::random().as_slice());

        let response = abci_client.finalize_block(request);

        // get newly made block from cache to recreate expected app hash
        let mut rw_lock = abci_client.block_cache.write().expect("should get lock");
        let sealed_block_with_peg = rw_lock.pop_newest().expect("to have block");
        let expected_app_hash =
            prost::bytes::Bytes::copy_from_slice(&sealed_block_with_peg.1.block().hash().0);

        let expected_response = ResponseFinalizeBlock {
            events: vec![],
            tx_results: vec![ExecTxResult { code: SUCCESS, data: ndd_bytes, ..Default::default() }],
            validator_updates: vec![],
            consensus_param_updates: None,
            app_hash: expected_app_hash,
        };

        assert_eq!(response, expected_response);
    }

    #[test]
    #[should_panic]
    fn test_finalize_block_with_signed_tx() {
        let abci_client = abci_client_builder();

        let mut request = RequestFinalizeBlock::default();

        // first tx should be non-deterministic data
        let ndd_bytes = abci_client.non_deterministic_data_bytes();

        // second tx should be a signed transaction
        let mut tx_generator = TransactionGenerator::new(thread_rng());
        let signed_tx = tx_generator.transaction().into_legacy();
        let mut buf = Vec::new();
        signed_tx.encode_enveloped(&mut buf);
        let signed_tx_bytes = prost::bytes::Bytes::copy_from_slice(buf.as_slice());

        request.txs = vec![ndd_bytes.clone(), signed_tx_bytes];

        let proposer_address = prost::bytes::Bytes::copy_from_slice(Address::ZERO.0.as_slice());
        request.proposer_address = proposer_address;

        request.time = Some(Timestamp::default());
        request.hash = prost::bytes::Bytes::copy_from_slice(FixedBytes::<32>::random().as_slice());

        // this should panic bc prevrandao isn't being set in the evm env during tests
        // but all the custom code is executed successfully up to `build_and_execute`
        let _response = abci_client.finalize_block(request);
    }

    // this needs to be an integration test bc the driver needs to be spun up as well
    // this requires a btc server to be running
    #[test]
    #[should_panic]
    fn test_commit() {
        let abci_client = abci_client_builder();

        let mut request = RequestFinalizeBlock::default();

        let ndd_bytes = abci_client.non_deterministic_data_bytes();

        request.txs = vec![ndd_bytes.clone()];

        let proposer_address = prost::bytes::Bytes::copy_from_slice(Address::ZERO.0.as_slice());
        request.proposer_address = proposer_address;

        request.time = Some(Timestamp::default());
        request.hash = prost::bytes::Bytes::copy_from_slice(FixedBytes::<32>::random().as_slice());

        // need to call finalize block first to generate a block in the cache to commit
        let _finalize_block_response = abci_client.finalize_block(request);

        // will panic bc the driver isn't running
        let _response = abci_client.commit();
    }
}
