/// The purpose of this module is to provide a bridge between the CometBFT and the EVM
/// application state
use std::{
    io,
    sync::{Arc, RwLock},
};

use btcserverlib::extended_client::BtcServerExtendedClient;
use bytes::Buf;
use reth_basic_payload_builder::{BuildArguments, PayloadConfig};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_consensus_common::utils::unix_timestamp;
use reth_eth_wire::NewBlock;
use reth_ethereum_payload_builder::default_ethereum_payload_builder;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_network::NetworkHandle;
use reth_node_ethereum::EthEngineTypes;
use tendermint_light_client::instance::Instance;
use thiserror::Error;

use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_primitives::{
    botanix::block_with_peg::SealedBlockWithPeg, Address, BlockHash, TransactionSigned,
};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, ExecutorFactory, StateProviderFactory};
use reth_revm::primitives::FixedBytes;
use reth_rpc_types::{engine::PayloadAttributes, BlockId};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::{EthPooledTransaction, EthTransactionValidator, TransactionPool};
use ruint::Uint;
use schnellru::{ByLength, LruMap};

use comet_bft_rpc::HttpCometBFTRpcClientFactory;

use tendermint_abci::{Application, ServerBuilder};
use tendermint_proto::{
    abci::{
        ExecTxResult, RequestExtendVote, RequestPrepareProposal, RequestProcessProposal,
        RequestVerifyVoteExtension, ResponseCommit, ResponseExtendVote, ResponsePrepareProposal,
        ResponseProcessProposal, ResponseVerifyVoteExtension,
    },
    v0_38::abci::{
        RequestCheckTx, RequestFinalizeBlock, RequestInfo, RequestInitChain, ResponseCheckTx,
        ResponseFinalizeBlock, ResponseInfo, ResponseInitChain,
    },
};

use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info, warn};

use crate::{
    builder::BitcoinCheckpoint, comet_bft::non_deterministic_data::NonDeterministicData,
    engine_util, excecution_utils::authority_execution_utils::build_and_execute,
    utils::call_notify_pegin, AuthorityConsensus, Storage,
};

use super::light_client::LightCBFTClientBuilder;

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
    btc_server: BtcServerExtendedClient,
    authority_consensus: AuthorityConsensus,
    to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
}

impl<EF, BF, DB> ABCIClientBuilder<EF, BF, DB>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: ExecutorFactory + Clone + 'static,
    BF: BitcoindFactory + Clone + 'static,
{
    pub fn new(
        storage: Storage<EF, BF, DB>,
        bitcoin_checkpoint: BitcoinCheckpoint,
        network_handle: NetworkHandle,
        btc_server: BtcServerExtendedClient,
        authority_consensus: AuthorityConsensus,
        to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
    ) -> Self {
        Self {
            storage,
            bitcoin_checkpoint,
            network_handle,
            btc_server,
            authority_consensus,
            to_engine,
        }
    }

    pub fn start_server<Pool: TransactionPool + Clone + 'static>(
        &self,
        task_executor: &impl TaskSpawner,
        validator: EthTransactionValidator<DB, EthPooledTransaction>,
        tx_pool: Pool,
        abci_port: u16,
        cometbft_rpc_port: u16,
    ) {
        let cbft_rpc_provider =
            HttpCometBFTRpcClientFactory::default().with_port(cometbft_rpc_port);
        let (driver_tx, driver_rx) = tokio::sync::mpsc::channel(100);

        let app = ABCIClient::new(
            self.storage.clone(),
            validator,
            tx_pool,
            self.bitcoin_checkpoint.clone(),
            driver_tx,
            cbft_rpc_provider.clone(),
        );
        let mut abci_driver = ABCIDriver::new(
            self.storage.clone(),
            cbft_rpc_provider.clone(),
            self.authority_consensus.clone(),
            self.btc_server.clone(),
            self.network_handle.clone(),
            driver_rx,
            self.to_engine.clone(),
        );

        let server_builder = ServerBuilder::default();
        // server will always bind to default address
        // CometBFT will always run on the same machine and container
        let server =
            server_builder.bind(format!("127.0.0.1:{abci_port}"), app).expect("build server");

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
                abci_driver.start().await;
            }),
        );
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
    cbft_rpc_provider: HttpCometBFTRpcClientFactory,
}

impl<EF, BF, DB, Pool> ABCIClient<EF, BF, DB, Pool>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: ExecutorFactory + Clone + 'static,
    BF: BitcoindFactory + Clone + 'static,
    Pool: TransactionPool + Clone + 'static,
{
    fn new(
        storage: Storage<EF, BF, DB>,
        validator: EthTransactionValidator<DB, EthPooledTransaction>,
        pool: Pool,
        bitcoin_checkpoint: BitcoinCheckpoint,
        driver_tx: tokio::sync::mpsc::Sender<ABCIDriverMessage>,
        cbft_rpc_provider: HttpCometBFTRpcClientFactory,
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
        }
    }

    /// Returns the payload builder config
    /// this method will block and wait for the storage lock
    fn payload_builder_arguments(&self) -> PayloadConfig<EthPayloadBuilderAttributes> {
        let storage = self.storage.inner.blocking_read();
        let client = storage.client.clone();
        let chain_spec = storage.chain_spec.clone();
        drop(storage); // Drop the lock

        let best_header =
            client.latest_header().expect("should have latest").expect("should have header");
        let best_block = BlockReaderIdExt::block_by_id(&client, BlockId::latest())
            .expect("have latest")
            .expect("have block")
            .seal(best_header.hash());

        // let builder_config = EthPayloadBuilderAttributes::new(best_block.hash(), );
        let payload_attributes = PayloadAttributes {
            // Attributes here dont really matter
            // We just want to build a payload with the best txs
            timestamp: unix_timestamp(),
            prev_randao: FixedBytes::<32>::random(),
            suggested_fee_recipient: Address::ZERO,
            withdrawals: None,
            parent_beacon_block_root: None,
        };

        let payload_builder_attributes =
            EthPayloadBuilderAttributes::new(best_block.hash(), payload_attributes);

        let payload_config = PayloadConfig::new(
            Arc::new(best_block),
            reth_primitives::Bytes::default(),
            payload_builder_attributes,
            chain_spec,
        );
        payload_config
    }

    pub fn aggregate_public_key(&self) -> secp256k1::PublicKey {
        self.storage.inner.blocking_read().aggregate_public_key.expect("agg pk exists")
    }

    pub fn bitcoin_blockhash(&self) -> bitcoin::BlockHash {
        self.bitcoin_checkpoint.blocking_read().expect("should have checkpoint").0.block_hash()
    }

    pub fn application_hash(&self, db: &impl BlockReaderIdExt) -> prost::bytes::Bytes {
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
    EF: ExecutorFactory + Clone + 'static,
    BF: BitcoindFactory + Clone + 'static,
    Pool: TransactionPool + Clone + 'static,
{
    // docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#init_chain
    fn init_chain(&self, _request: RequestInitChain) -> ResponseInitChain {
        info!("init_chain request: {:?}", _request);
        let client = self.storage.client.clone();
        let res =
            ResponseInitChain { app_hash: self.application_hash(&client), ..Default::default() };

        res
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#info
    fn info(&self, request: RequestInfo) -> ResponseInfo {
        info!("info request: {:?}", request);
        let client = self.storage.client.clone();
        let latest_header =
            client.latest_header().expect("should have latest").expect("should have header");

        let info_res = ResponseInfo {
            data: String::default(),
            version: "TODO".to_string(),
            app_version: 1,
            last_block_height: latest_header.number as i64,
            last_block_app_hash: self.application_hash(&client),
        };

        info_res
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#prepareProposal
    fn prepare_proposal(&self, request: RequestPrepareProposal) -> ResponsePrepareProposal {
        info!("prepare_proposal request: {:?}", request);
        let _txs_bytes = request.txs;

        let payload_config = self.payload_builder_arguments();
        let client = self.storage.inner.blocking_read().client.clone();

        let res = default_ethereum_payload_builder(BuildArguments {
            client,
            pool: self.pool.clone(),
            cached_reads: Default::default(),
            config: payload_config,
            cancel: Default::default(),
            best_payload: None,
        });

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
                        info!("prepare_proposal response: {:?}", txs);

                        let non_deterministic_data = NonDeterministicData::new(
                            self.bitcoin_blockhash(),
                            self.aggregate_public_key(),
                        );

                        // inject non-deterministic data tx at index 0 so historical sync will pass verification
                        let non_deterministic_data_bytes = prost::bytes::Bytes::copy_from_slice(
                            non_deterministic_data
                                .serialize()
                                .expect("non deterministic data to be serialized")
                                .as_slice(),
                        );
                        txs.insert(0, non_deterministic_data_bytes);

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
        let tx_bytes = request.tx.clone();
        let hex = hex::decode(tx_bytes.clone()).unwrap();

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
                            error = (ERROR, "Error occured while validating transaction");
                        }
                        reth_transaction_pool::TransactionValidationOutcome::Error(_, e) => {
                            error!("TxError: Error validating transaction: {:?}", e);
                            error = (ERROR, "Error occured while validating transaction");
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

        ResponseCheckTx {
            code: error.0,
            log: error.1.to_string(),
            info: error.1.to_string(),
            ..Default::default()
        }
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#prepareproposal
    fn process_proposal(&self, request: RequestProcessProposal) -> ResponseProcessProposal {
        info!("process_proposal request: {:?}", request);
        let storage = self.storage.inner.blocking_read();

        if storage.aggregate_public_key.is_none() {
            warn!("Aggregate public key is not set in process proposal");
            return ResponseProcessProposal { status: VERIFY_REJECT };
        }

        // Extract the actual txs
        let txs_bytes = request.txs;
        // Extract who built this block
        let block_builder_address = Address::new(
            FixedBytes::<20>::from_slice(request.proposer_address.to_vec().as_slice()).0,
        );

        // Extract block time: this must come from the CBFT block header NOT the system time
        // As that will be underministic
        let block_time = request.time.expect("block time");
        let cbft_block_hash = FixedBytes::<32>::from_slice(&request.hash.to_vec().as_slice());

        // extract first tx which contains non-deterministic data and validate
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

        let bitcoin_checkpoint_block_hash = self
            .bitcoin_checkpoint
            .blocking_read()
            .expect("should have checkpoint")
            .clone()
            .0
            .block_hash();

        // check non-deterministic data: btc block hash and aggregate public key
        if bitcoin_checkpoint_block_hash != non_deterministic_data.bitcoin_block_hash {
            warn!("Bitcoin block hash mismatch");
            return ResponseProcessProposal { status: VERIFY_REJECT };
        }

        let agg_pk = storage.aggregate_public_key.expect("to be defined by now");
        if agg_pk != non_deterministic_data.aggregated_public_key {
            warn!("Aggregate public key mismatch");
            return ResponseProcessProposal { status: VERIFY_REJECT };
        }

        // TODO if we fail to decode the txs, we should reject the block
        let txs = txs_bytes
            .iter()
            .skip(1) // skip non-deterministic data tx
            .map(|tx| {
                let signed_tx =
                    TransactionSigned::decode_enveloped(&mut tx.to_vec().as_slice()).unwrap();
                signed_tx
            })
            .collect::<Vec<_>>();

        match build_and_execute(
            txs,
            storage.chain_spec.clone(),
            &block_builder_address,
            storage.evm_config,
            &storage.client,
            &storage.bitcoind_factory,
            storage.btc_network,
            &bitcoin_checkpoint_block_hash,
            &agg_pk,
            &storage.authorities,
            block_time,
        ) {
            Ok(sealed_block_with_peg) => {
                info!(
                    "Block built successfully, resulting block hash: {:?}",
                    sealed_block_with_peg.block().hash()
                );

                self.block_cache.write().unwrap().insert(cbft_block_hash, sealed_block_with_peg);
            }
            Err(e) => {
                error!("Error building block: {:?}", e);
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        }
        ResponseProcessProposal { status: VERIFY_ACCEPTED }
    }

    ///docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#finalizeblock
    fn finalize_block(&self, request: RequestFinalizeBlock) -> ResponseFinalizeBlock {
        info!("finalize_block request: {:?}", request);
        let cbft_block_hash = FixedBytes::<32>::from_slice(&request.hash.to_vec().as_slice());
        // If this block does not exist in the cache, we should panic
        let sealed_block_with_peg = self
            .block_cache
            .write()
            .expect("should get write lock")
            .get(&cbft_block_hash)
            .expect("block to exist")
            .clone();

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
        ResponseFinalizeBlock {
            events: vec![],
            tx_results: exec_results,
            validator_updates: vec![],
            consensus_param_updates: None,
            app_hash: prost::bytes::Bytes::copy_from_slice(&block_hash.0),
        }
    }

    fn commit(&self) -> ResponseCommit {
        let candidate_blocks = self.block_cache.write().unwrap();

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

        ResponseCommit::default()
    }
}

enum ABCIDriverMessage {
    /// Finalize a block, message includes the sealed block and the CBFT block hash
    CommitBlock((SealedBlockWithPeg, FixedBytes<32>, tokio::sync::oneshot::Sender<()>)),
    Exit,
}

// The driver is mainly responsible for driving block completion and finalization
// Once a finalize block is recieved the drive is reponsible for
// * Updating the canonical chain via DB
// * Sending the finalized block to the network
// * Sending the finalized block to the engine (FCU)
// * Sending pegins / pegouts to the btc server
// * Updating the [ExtraDataHeader] with the block witnesses
pub(crate) struct ABCIDriver<EF, BF, DB> {
    storage: Storage<EF, BF, DB>,
    cbft_rpc_provider: HttpCometBFTRpcClientFactory,
    authority_consensus: AuthorityConsensus,
    btc_server: BtcServerExtendedClient,
    network_handle: NetworkHandle,
    driver_rx: tokio::sync::mpsc::Receiver<ABCIDriverMessage>,
    to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
}

impl<EF, BF, DB> ABCIDriver<EF, BF, DB>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: ExecutorFactory + Clone + 'static,
    BF: BitcoindFactory + Clone + 'static,
{
    fn new(
        storage: Storage<EF, BF, DB>,
        cbft_rpc_provider: HttpCometBFTRpcClientFactory,
        authority_consensus: AuthorityConsensus,
        btc_server: BtcServerExtendedClient,
        network_handle: NetworkHandle,
        driver_rx: tokio::sync::mpsc::Receiver<ABCIDriverMessage>,
        to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
    ) -> Self {
        Self {
            storage,
            cbft_rpc_provider,
            authority_consensus,
            btc_server,
            network_handle,
            driver_rx,
            to_engine,
        }
    }

    async fn start(&mut self) {
        loop {
            if let Some(message) = self.driver_rx.recv().await {
                match message {
                    ABCIDriverMessage::CommitBlock((sealed_block_with_peg, cbft_hash, tx)) => {
                        let client = self.storage.client.clone();
                        let sealed_block_with_senders = sealed_block_with_peg.block();
                        let sealed_header = sealed_block_with_senders.header.clone();
                        let block_hash = sealed_header.hash();

                        // Update canonical chain
                        match client.insert_block(
                            sealed_block_with_senders.clone(),
                            reth_interfaces::blockchain_tree::BlockValidationKind::Exhaustive,
                        ) {
                            Ok(_) => {}
                            Err(e) => {
                                error!(target: "consensus::authority", ?e, "Failed to insert block");
                                // TODO handle error here
                            }
                        }
                        client.set_canonical_head(sealed_block_with_senders.header.clone());
                        client.set_safe(sealed_block_with_senders.header.clone());
                        client.set_finalized(sealed_block_with_senders.header.clone());

                        engine_util::send_fork_choice_update_payload(
                            block_hash,
                            self.to_engine.clone(),
                        )
                        .await
                        .unwrap();

                        // Annount to the network
                        let block_to_commit = sealed_block_with_senders.block.clone().unseal();
                        self.network_handle.announce_block(
                            NewBlock { block: block_to_commit, td: Uint::ZERO },
                            block_hash,
                        );

                        let pegins = sealed_block_with_peg
                            .pegins()
                            .iter()
                            .map(|p| p.meta.clone())
                            .flatten()
                            .collect::<Vec<_>>();
                        // TODO what happens if the pegins fail? Should we panic? Should this be
                        // called in commit?
                        call_notify_pegin(&mut self.btc_server, &pegins)
                            .await
                            .expect("Should notify pegins");

                        tx.send(()).expect("to send");
                    }
                    ABCIDriverMessage::Exit => {
                        break;
                    }
                }
            }
        }
    }
}
