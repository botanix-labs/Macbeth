use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_provider::{BlockReaderIdExt, CanonChainTracker, ExecutorFactory, StateProviderFactory};
use reth_tasks::TaskSpawner;
/// The purpose of this module is to provide a bridge between the CometBFT and the EVM
/// application state
use tendermint_abci::{Application, Server, ServerBuilder};
use tendermint_proto::v0_38::abci::{
    Event, EventAttribute, RequestCheckTx, RequestFinalizeBlock, RequestInfo, RequestQuery,
    ResponseCheckTx, ResponseCommit, ResponseFinalizeBlock, ResponseInfo, ResponseQuery,
};
use tracing::info;

use crate::Storage;

/// The size of the read buffer for each incoming connection to the ABCI
/// server (1MB).
pub const DEFAULT_SERVER_READ_BUF_SIZE: usize = 1024 * 1024;

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

    pub fn start_server(&self, task_executor: &impl TaskSpawner) {
        let app = ABCIClient::new(self.storage.clone());
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
pub struct ABCIClient<EF, BF, DB> {
    storage: Storage<EF, BF, DB>,
}

impl<EF, BF, DB> ABCIClient<EF, BF, DB>
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
}

impl<EF, BF, DB> Application for ABCIClient<EF, BF, DB>
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
    // TODO define the ABCI methods
    fn info(&self, request: RequestInfo) -> ResponseInfo {
        info!("info request: {:?}", request);
        ResponseInfo::default()
    }
}
