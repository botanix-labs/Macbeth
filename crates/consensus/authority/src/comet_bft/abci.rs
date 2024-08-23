use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_primitives::{Transaction, TransactionSigned, TxEip1559};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, ExecutorFactory, StateProviderFactory};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::{
    EthPoolTransaction, EthPooledTransaction, EthTransactionValidator, PoolTransaction,
};
use serde::{Deserialize, Serialize};
/// The purpose of this module is to provide a bridge between the CometBFT and the EVM
/// application state
use tendermint_abci::{Application, Server, ServerBuilder};
use tendermint_proto::v0_38::abci::{
    Event, EventAttribute, RequestCheckTx, RequestFinalizeBlock, RequestInfo, RequestQuery,
    ResponseCheckTx, ResponseCommit, ResponseFinalizeBlock, ResponseInfo, ResponseQuery,
};
use tracing::{error, info};

use crate::Storage;

/// The size of the read buffer for each incoming connection to the ABCI
/// server (1MB).
pub const DEFAULT_SERVER_READ_BUF_SIZE: usize = 1024 * 1024;

/// Consts
pub const SUCCESS: u32 = 0;
pub const ERROR: u32 = 1;

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

    pub fn start_server<T: EthPoolTransaction + Clone>(
        &self,
        task_executor: &impl TaskSpawner,
        validator: EthTransactionValidator<DB, T>,
    ) {
        let app = ABCIClient::new(self.storage.clone(), validator);
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

#[derive(Clone, Debug)]
pub struct ABCIClient<EF, BF, DB, T> {
    storage: Storage<EF, BF, DB>,
    validator: EthTransactionValidator<DB, T>,
}

impl<EF, BF, DB, T> ABCIClient<EF, BF, DB, T>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: ExecutorFactory + Clone + 'static,
    BF: BitcoindFactory + Clone + 'static,
    T: EthPoolTransaction + Clone + 'static,
{
    pub fn new(storage: Storage<EF, BF, DB>, validator: EthTransactionValidator<DB, T>) -> Self {
        Self { storage, validator }
    }
}

impl<EF, BF, DB, T> Application for ABCIClient<EF, BF, DB, T>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: ExecutorFactory + Clone + 'static,
    BF: BitcoindFactory + Clone + 'static,
    T: EthPoolTransaction + Clone + 'static,
{
    // docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#info
    fn info(&self, request: RequestInfo) -> ResponseInfo {
        info!("info request: {:?}", request);
        ResponseInfo::default()
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#checktx
    fn check_tx(&self, request: RequestCheckTx) -> ResponseCheckTx {
        // We are ignore type for now
        // One of CheckTx_New or CheckTx_Recheck. CheckTx_New is the default and means that a full
        // check of the tranasaction is required. CheckTx_Recheck types are used when the mempool is
        // initiating a normal recheck of a transaction.
        let _type = request.r#type;
        let mut tx_bytes = request.tx.clone();

        let mut error = (SUCCESS, "Ok");
        match TransactionSigned::decode_enveloped(&mut tx_bytes.to_vec().as_slice()) {
            Ok(tx) => {
                if let Ok(tx_ec_recover) = tx.try_into_ecrecovered() {
                    let pool_tx = EthPooledTransaction::new(
                        tx_ec_recover,
                        tx_ec_recover.length_without_header(),
                    );

                    let res = self.validator.validate_one(
                        reth_transaction_pool::TransactionOrigin::External,
                        pool_tx.clone(),
                    );

                    match res {
                        reth_transaction_pool::TransactionValidationOutcome::Valid {
                            balance,
                            state_nonce,
                            transaction,
                            propagate,
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
                    error = (ERROR, &format!("Error recovering tx signer. Invalid signature"));
                }
            }
            Err(e) => {
                error!("Error decoding transaction: {:?}", e);
                error = (ERROR, "Error decoding transaction");
            }
        }

        ResponseCheckTx { code: error.0, log: error.1.to_string(), ..Default::default() }
    }
}
