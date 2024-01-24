use crate::{epoch_manager::EpochManager, Storage};
use reth_beacon_consensus::BeaconEngineMessage;

use reth_btc_wallet::block_source::MempoolSpace;

use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_network::NetworkHandle;
use reth_primitives::ChainSpec;
use reth_provider::{
    BlockReaderIdExt, CanonChainTracker, CanonStateNotificationSender, StateProviderFactory,
};
use reth_stages::PipelineEvent;
use reth_tasks::TaskExecutor;

use reth_payload_builder::PayloadBuilderHandle;

use secp256k1::{All, Secp256k1};
use std::sync::Arc;

use tokio::sync::{mpsc::UnboundedSender, RwLock};
use tokio_stream::wrappers::UnboundedReceiverStream;

use client::BtcServerClient;

pub struct BlockProductionTask<Client> {
    /// The configured chain spec
    pub(crate) chain_spec: Arc<ChainSpec>,
    /// The active epoch
    pub(crate) epoch_manager: EpochManager<Client>,
    /// Shared storage to insert new blocks
    pub(crate) storage: Storage<Client>,
    /// TODO: ideally this would just be a sender of hashes
    pub(crate) to_engine: UnboundedSender<BeaconEngineMessage>,
    /// The pipeline events to listen on
    pub(crate) pipe_line_events: Option<UnboundedReceiverStream<PipelineEvent>>,
    /// BTC Server client
    pub(crate) btc_server: BtcServerClient<tonic::transport::Channel>,
    /// Recent bitcoin block headers
    pub(crate) bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    /// Bitcoin block source
    pub(crate) bitcoin_block_source: MempoolSpace,
    /// Instance of secp
    pub(crate) secp: Secp256k1<All>,
    /// Key of authority
    pub(crate) sk: secp256k1::SecretKey,
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    /// Task executor
    #[allow(dead_code)]
    task_executor: TaskExecutor,
    /// Payload store
    pub payload_store: PayloadBuilderHandle,
}

impl<Client> BlockProductionTask<Client>
where
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        to_engine: UnboundedSender<BeaconEngineMessage>,
        canon_state_notification: CanonStateNotificationSender,
        storage: Storage<Client>,
        btc_server: BtcServerClient<tonic::transport::Channel>,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        bitcoin_block_source: MempoolSpace,
        secp: Secp256k1<All>,
        sk: secp256k1::SecretKey,
        epoch_manager: EpochManager<Client>,
        network_handle: NetworkHandle,
        task_executor: TaskExecutor,
        payload_store: PayloadBuilderHandle,
    ) -> Self {
        Self {
            chain_spec,
            storage,
            to_engine,
            pipe_line_events: None,
            btc_server,
            bitcoin_block_header,
            bitcoin_block_source,
            secp,
            sk,
            epoch_manager,
            network_handle,
            task_executor,
            payload_store,
        }
    }

    pub async fn start_task(&mut self) -> () {
        loop {
            self.try_build_block().await;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    /// Sets the pipeline events to listen on.
    pub fn set_pipeline_events(&mut self, events: UnboundedReceiverStream<PipelineEvent>) {
        self.pipe_line_events = Some(events);
    }
}

impl<Client> std::fmt::Debug for BlockProductionTask<Client> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockProductionTask").finish_non_exhaustive()
    }
}
