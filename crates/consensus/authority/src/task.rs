use crate::{epoch_manager::EpochManager, Storage};
use reth_beacon_consensus::BeaconEngineMessage;

use reth_btc_wallet::block_source::MempoolSpace;

use crate::sync::SyncController;

use reth_network::{message::NewBlockMessage, NetworkEvents, NetworkHandle};
use reth_primitives::{BlockBody, ChainSpec, SealedBlockWithSenders};
use reth_provider::{
    BlockReaderIdExt, BundleStateWithReceipts, CanonChainTracker, CanonStateNotificationSender,
    Chain, StateProviderFactory,
};
use reth_stages::PipelineEvent;
use reth_tasks::TaskExecutor;
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};

use secp256k1::{All, Secp256k1};
use std::{collections::VecDeque, sync::Arc};

use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    RwLock,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

use url::Url;

use client::BtcServerClient;

pub struct BlockProductionTask<Client, Pool: TransactionPool> {
    /// The configured chain spec
    pub(crate) chain_spec: Arc<ChainSpec>,
    /// The client used to interact with the state
    /// Note this is a database client
    pub(crate) client: Client,
    /// The active epoch
    pub(crate) epoch_manager: EpochManager<Client>,
    /// Shared storage to insert new blocks
    pub(crate) storage: Storage<Client>,
    /// Pool where transactions are stored
    pub(crate) pool: Pool,
    /// backlog of sets of transactions ready to be mined
    pub(crate) queued:
        VecDeque<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>,
    /// TODO: ideally this would just be a sender of hashes
    pub(crate) to_engine: UnboundedSender<BeaconEngineMessage>,
    /// Used to notify consumers of new blocks
    pub(crate) canon_state_notification: CanonStateNotificationSender,
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
    /// Events from block import
    pub(crate) block_import_rx: UnboundedReceiver<NewBlockMessage>,
    /// Task executor
    #[allow(dead_code)]
    task_executor: TaskExecutor,
}

impl<Client, Pool: TransactionPool> BlockProductionTask<Client, Pool>
where
    Client: BlockReaderIdExt + StateProviderFactory + CanonChainTracker + Clone + 'static,
    Pool: TransactionPool,
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        to_engine: UnboundedSender<BeaconEngineMessage>,
        canon_state_notification: CanonStateNotificationSender,
        storage: Storage<Client>,
        client: Client,
        pool: Pool,
        btc_server: BtcServerClient<tonic::transport::Channel>,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        bitcoin_block_source_address: Url,
        secp: Secp256k1<All>,
        sk: secp256k1::SecretKey,
        epoch_manager: EpochManager<Client>,
        network_handle: NetworkHandle,
        block_import_rx: UnboundedReceiver<NewBlockMessage>,
        task_executor: TaskExecutor,
    ) -> Self {
        Self {
            chain_spec,
            client,
            storage,
            pool,
            to_engine,
            canon_state_notification,
            queued: Default::default(),
            pipe_line_events: None,
            btc_server,
            bitcoin_block_header,
            bitcoin_block_source: MempoolSpace::new(bitcoin_block_source_address.to_string()),
            secp,
            sk,
            epoch_manager,
            network_handle,
            block_import_rx,
            task_executor,
        }
    }

    pub async fn start_task(&mut self) -> () {
        let mut sync_controller = SyncController::new(
            self.network_handle.event_listener(),
            self.network_handle.peer_id().clone(),
            self.to_engine.clone(),
        );

        self.task_executor.spawn_critical(
            "Sync Controller",
            Box::pin(async move {
                sync_controller.try_sync_peer_tip().await;
            }),
        );

        loop {
            self.try_fetch_block().await;
            self.try_build_block().await;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    /// Sets the pipeline events to listen on.
    pub fn set_pipeline_events(&mut self, events: UnboundedReceiverStream<PipelineEvent>) {
        self.pipe_line_events = Some(events);
    }
}

impl<Client, Pool: TransactionPool> std::fmt::Debug for BlockProductionTask<Client, Pool> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockProductionTask").finish_non_exhaustive()
    }
}
