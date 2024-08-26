/// The purpose of this module is to provide a bridge between the CometBFT and the EVM
/// application state
use std::sync::{Arc, RwLock};

use bitcoin::hashes::{sha256, Hash};
use btcserverlib::extended_client::BtcServerExtendedClient;
use reth_basic_payload_builder::{BuildArguments, PayloadConfig};
use reth_btc_wallet::{address::EthAddress, bitcoind::BitcoindFactory};
use reth_consensus_common::utils::unix_timestamp;
use reth_eth_wire::NewBlock;
use reth_ethereum_payload_builder::default_ethereum_payload_builder;
use reth_interfaces::blockchain_tree::{BlockValidationKind::Exhaustive, BlockchainTreeEngine};
use reth_network::NetworkHandle;
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_primitives::{Address, Block, BlockHash, SealedBlockWithSenders, TransactionSigned};
use reth_provider::{
    BlockReaderIdExt, BundleStateWithReceipts, CanonChainTracker, ExecutorFactory,
    StateProviderFactory,
};
use reth_revm::primitives::FixedBytes;
use reth_rpc_types::{engine::PayloadAttributes, BlockId};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::{
    EthPooledTransaction, EthTransactionValidator, TransactionOrigin, TransactionPool,
};
use schnellru::{ByLength, LruMap};

use comet_bft_rpc::HttpCometBFTRpcClientFactory;
use ruint::Uint;
use tendermint_abci::{Application, ServerBuilder};
use tendermint_proto::{
    abci::{
        RequestPrepareProposal, RequestProcessProposal, ResponsePrepareProposal,
        ResponseProcessProposal,
    },
    v0_38::abci::{
        RequestCheckTx, RequestFinalizeBlock, RequestInfo, RequestInitChain, ResponseCheckTx,
        ResponseFinalizeBlock, ResponseInfo, ResponseInitChain,
    },
};

use tracing::{error, info};

use crate::{
    builder::BitcoinCheckpoint, engine_util,
    excecution_utils::authority_execution_utils::build_and_execute, AuthorityConsensus, Storage,
};

/// Consts
const SUCCESS: u32 = 0;
const ERROR: u32 = 1;

// https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#verifystatus
const VERIFY_UNKNOWN: i32 = 0;
const VERIFY_ACCEPTED: i32 = 1;
const VERIFY_REJECT: i32 = 2;

#[derive(Clone, Debug)]
pub struct ABCIClientBuilder<EF, BF, DB> {
    storage: Storage<EF, BF, DB>,
    bitcoin_checkpoint: BitcoinCheckpoint,
    network_handle: NetworkHandle,
    btc_server: BtcServerExtendedClient,
    authority_consensus: AuthorityConsensus,
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
    ) -> Self {
        Self { storage, bitcoin_checkpoint, network_handle, btc_server, authority_consensus }
    }

    pub fn start_server<Pool: TransactionPool + Clone + 'static>(
        &self,
        task_executor: &impl TaskSpawner,
        validator: EthTransactionValidator<DB, EthPooledTransaction>,
        tx_pool: Pool,
    ) {
        let cbft_rpc_provider = HttpCometBFTRpcClientFactory::default();
        let app = ABCIClient::new(
            self.storage.clone(),
            validator,
            cbft_rpc_provider,
            tx_pool,
            self.bitcoin_checkpoint.clone(),
            self.authority_consensus.clone(),
            self.btc_server.clone(),
            self.network_handle.clone(),
        );
        let server_builder = ServerBuilder::default();
        // server will always bind to default address
        // CometBFT will always run on the same machine and container
        let server = server_builder.bind("127.0.0.1:26658", app).expect("build server");

        task_executor.spawn_critical(
            "ABCI Client",
            Box::pin(async move {
                // we should panic here if cannot launch the ABCI server
                server.listen().expect("to start server");
            }),
        );
    }
}

#[derive(Debug, Clone)]
pub struct ABCIClient<EF, BF, DB, Pool> {
    storage: Storage<EF, BF, DB>,
    validator: EthTransactionValidator<DB, EthPooledTransaction>,
    cbft_rpc_provider: HttpCometBFTRpcClientFactory,
    pool: Pool,
    bitcoin_checkpoint: BitcoinCheckpoint,
    authority_consensus: AuthorityConsensus,
    btc_server: BtcServerExtendedClient,
    network_handle: NetworkHandle,
    block_cache: Arc<RwLock<LruMap<BlockHash, SealedBlockWithSenders>>>,
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
        cbft_rpc_provider: HttpCometBFTRpcClientFactory,
        pool: Pool,
        bitcoin_checkpoint: BitcoinCheckpoint,
        authority_consensus: AuthorityConsensus,
        btc_server: BtcServerExtendedClient,
        network_handle: NetworkHandle,
    ) -> Self {
        Self {
            storage,
            validator,
            cbft_rpc_provider,
            pool,
            bitcoin_checkpoint,
            authority_consensus,
            btc_server,
            network_handle,
            // Saving the last 5 blocks that were proposed
            block_cache: Arc::new(RwLock::new(LruMap::new(ByLength::new(5)))),
        }
    }

    /// Returns the payload builder config
    /// this method will block and wait for the storage lock
    fn payload_builder_arguements(&self) -> PayloadConfig<EthPayloadBuilderAttributes> {
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
    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#info
    fn info(&self, request: RequestInfo) -> ResponseInfo {
        info!("info request: {:?}", request);
        ResponseInfo::default()
    }

    // docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#init_chain
    fn init_chain(&self, _request: RequestInitChain) -> ResponseInitChain {
        info!("init_chain request: {:?}", _request);
        // TODO lets get rid of blocking read here
        let client = self.storage.inner.blocking_read().client.clone();
        let state_root = client
            .latest_header()
            .expect("should have latest")
            .expect("should have header")
            .state_root;
        let res = ResponseInitChain {
            app_hash: bytes::Bytes::copy_from_slice(&state_root.0),
            ..Default::default()
        };

        res
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#prepareProposal
    fn prepare_proposal(&self, request: RequestPrepareProposal) -> ResponsePrepareProposal {
        info!("prepare_proposal request: {:?}", request);
        let _txs_bytes = request.txs;

        let payload_config = self.payload_builder_arguements();
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
                        let txs = block
                            .raw_transactions()
                            .iter()
                            .map(|tx| prost::bytes::Bytes::copy_from_slice(tx))
                            .collect::<_>();
                        info!("prepare_proposal response: {:?}", txs);
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
                            error!("Error validating transaction: {:?}", e);
                            error = (ERROR, "Error occured while validating transaction");
                        }
                        reth_transaction_pool::TransactionValidationOutcome::Error(_, e) => {
                            error!("Error validating transaction: {:?}", e);
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

    fn process_proposal(&self, request: RequestProcessProposal) -> ResponseProcessProposal {
        info!("process_proposal request: {:?}", request);
        // Extract the actual txs
        let txs_bytes = request.txs;

        if txs_bytes.is_empty() {
            // no need to execute anything
            return ResponseProcessProposal { status: VERIFY_ACCEPTED };
        }
        // Extract who built this block
        let block_builder_address = Address::new(
            FixedBytes::<20>::from_slice(request.proposer_address.to_vec().as_slice()).0,
        );

        // Extract block time: this must come from the CBFT block header NOT the system time
        // As that will be underministic
        let block_time = request.time.expect("block time");
        let cbft_block_hash = FixedBytes::<32>::from_slice(&request.hash.to_vec().as_slice());

        // TODO if we fail to decode the txs, we should reject the block
        let txs = txs_bytes
            .iter()
            .map(|tx| {
                let signed_tx =
                    TransactionSigned::decode_enveloped(&mut tx.to_vec().as_slice()).unwrap();
                signed_tx
            })
            .collect::<Vec<_>>();
        let storage = self.storage.inner.blocking_read();
        // TODO This should be coming from the results of the vote for this block
        let bitcoin_checkpoint_block_hash = self
            .bitcoin_checkpoint
            .blocking_read()
            .expect("should have checkpoint")
            .clone()
            .0
            .block_hash();

        match build_and_execute(
            txs,
            storage.chain_spec.clone(),
            &block_builder_address,
            storage.evm_config,
            &storage.client,
            &storage.bitcoind_factory,
            storage.btc_network,
            &bitcoin_checkpoint_block_hash,
            &storage.aggregate_public_key.expect("to be defined by now"),
            &storage.authorities,
            block_time,
        ) {
            Ok(sealed_block_with_senders) => {
                info!(
                    "Block built successfully, resulting block hash: {:?}",
                    sealed_block_with_senders.hash()
                );
                self.block_cache
                    .write()
                    .unwrap()
                    .insert(cbft_block_hash, sealed_block_with_senders);
            }
            Err(e) => {
                error!("Error building block: {:?}", e);
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        }
        ResponseProcessProposal { status: VERIFY_ACCEPTED }
    }

    fn finalize_block(&self, request: RequestFinalizeBlock) -> ResponseFinalizeBlock {
        // info!("finalize_block request: {:?}", request);
        // let block_hash = request.hash;
        // // If this block does not exist in the cache, we should panic
        // let block = self.block_cache.read().get(&block_hash).expect("block to exist").clone();

        // let mut block_to_commit = Block {
        //     header: sealed_header.clone().unseal(),
        //     body: txs,
        //     ommers: vec![],
        //     withdrawals: None,
        // };
        // // TODOs
        // // Send canon notif
        // // send fcu
        // // get block sigs via rpc and add to edh
        // // TODO extract pegins and send to btc server here
        // // TODO Extract block signatures via rpc

        // // Update canonical chain
        // let sealed_block = block_to_commit.clone().seal(sealed_header.hash());
        // let sealed_block_with_senders = sealed_block.try_seal_with_senders().unwrap();

        // match storage.client.insert_block(sealed_block_with_senders.clone(), Exhaustive) {
        //     Ok(_) => {}
        //     Err(e) => {
        //         error!(target: "consensus::authority", ?e, "Failed to insert block");
        //         // TODO handle error here
        //     }
        // }
        // storage.client.set_canonical_head(sealed_block.header.clone());
        // storage.client.set_safe(sealed_block.header.clone());
        // storage.client.set_finalized(sealed_block.header.clone());

        // engine_util::send_fork_choice_update_payload(sealed_header.hash(), to_engine)

        // // Annount to the network

        // let block_hash = sealed_header.hash();
        // self.network_handle
        //     .announce_block(NewBlock { block: block_to_commit, td: Uint::ZERO }, block_hash);

        ResponseFinalizeBlock::default()
    }
}
