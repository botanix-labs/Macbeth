/// The purpose of this module is to provide a bridge between the CometBFT and the EVM
/// application state
use std::sync::Arc;

use reth_basic_payload_builder::{BuildArguments, PayloadConfig};
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_consensus_common::utils::unix_timestamp;
use reth_ethereum_payload_builder::default_ethereum_payload_builder;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;

use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_primitives::{Address, TransactionSigned};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, ExecutorFactory, StateProviderFactory};
use reth_revm::primitives::FixedBytes;
use reth_rpc_types::{engine::PayloadAttributes, BlockId};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::{
    EthPooledTransaction, EthTransactionValidator, TransactionOrigin, TransactionPool,
};

use comet_bft_rpc::HttpCometBFTRpcClientFactory;
use tendermint_abci::{Application, ServerBuilder};
use tendermint_proto::{
    abci::{RequestPrepareProposal, ResponsePrepareProposal},
    v0_38::abci::{
        RequestCheckTx, RequestFinalizeBlock, RequestInfo, RequestInitChain, ResponseCheckTx,
        ResponseFinalizeBlock, ResponseInfo, ResponseInitChain,
    },
};
use tracing::{error, info};

use crate::Storage;

/// Consts
const SUCCESS: u32 = 0;
const ERROR: u32 = 1;

#[derive(Clone, Debug)]
pub struct ABCIClientBuilder<EF, BF, DB> {
    storage: Storage<EF, BF, DB>,
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
    pub fn new(storage: Storage<EF, BF, DB>) -> Self {
        Self { storage }
    }

    pub fn start_server<Pool: TransactionPool + Clone + 'static>(
        &self,
        task_executor: &impl TaskSpawner,
        validator: EthTransactionValidator<DB, EthPooledTransaction>,
        tx_pool: Pool,
    ) {
        let cbft_rpc_provider = HttpCometBFTRpcClientFactory::default();
        let app = ABCIClient::new(self.storage.clone(), validator, cbft_rpc_provider, tx_pool);
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
    pub fn new(
        storage: Storage<EF, BF, DB>,
        validator: EthTransactionValidator<DB, EthPooledTransaction>,
        cbft_rpc_provider: HttpCometBFTRpcClientFactory,
        pool: Pool,
    ) -> Self {
        Self { storage, validator, cbft_rpc_provider, pool }
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
        let txs_bytes = request.txs;
        // TODO can the txs be an invalid format here? If so, how do we handle it
        // let txs = txs_bytes
        //     .iter()
        //     .map(|tx| {
        //         let signed_tx =
        //             TransactionSigned::decode_enveloped(&mut tx.to_vec().as_slice()).unwrap();
        //         let ec_recovered_tx = signed_tx.try_into_ecrecovered().unwrap();
        //         let length = ec_recovered_tx.length_without_header();
        //         let pool_tx = EthPooledTransaction::new(ec_recovered_tx, length);
        //         (TransactionOrigin::External, pool_tx)
        //     })
        //     .collect::<Vec<_>>();

        // let res = self.validator.validate_all(txs);
        // TODO We are essentially ignoring the txs in the req here, what if there is a discrepenacy
        // should we add those Tx to the pool first?

        // info!("prepare_proposal response: {:?}", res);
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

    fn finalize_block(&self, request: RequestFinalizeBlock) -> ResponseFinalizeBlock {
        info!("finalize_block request: {:?}", request);
        ResponseFinalizeBlock::default()
    }
}
