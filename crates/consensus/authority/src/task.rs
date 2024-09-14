use std::sync::Arc;

use btcserverlib::extended_client::BtcServerExtendedClient;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_blockchain_tree_api::BlockchainTreeEngine;
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_evm::execute::BlockExecutorProvider;
use reth_network::{frost::manager::ToFrostManager, NetworkHandle};
use reth_node_api::EngineTypes;
use reth_node_ethereum::EthEngineTypes;
use reth_payload_builder::PayloadBuilderHandle;
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use reth_stages::PipelineEvent;
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    RwLock,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::{
    epoch_manager::EpochManager, frost_task::FrostNotificationMessage,
    pbft_task::PbftNotificationMessage, utxo_sync::UTXOSyncEngine, AuthorityConsensus, Storage,
};

pub struct BlockProductionTask<EF, BF, DB, Engine: EngineTypes, ToFrostMan> {
    /// The authority consensus wrapper
    pub(crate) consensus: AuthorityConsensus,
    /// The active epoch
    pub(crate) epoch_manager: EpochManager<EF, BF, DB>,
    /// Shared storage to insert new blocks
    pub(crate) storage: Storage<EF, BF, DB>,
    /// To engine sender
    pub(crate) to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
    /// The pipeline events to listen on
    pub(crate) pipe_line_events: Option<UnboundedReceiverStream<PipelineEvent>>,
    /// BTC Server client
    pub(crate) btc_server: BtcServerExtendedClient,
    /// Recent bitcoin block headers
    pub(crate) bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    /// Key of authority
    pub(crate) sk: secp256k1::SecretKey,
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    /// Ethereum Payload Builder
    pub(crate) payload_builder: PayloadBuilderHandle<EthEngineTypes>,
    /// Frost Task Receiver
    pub(crate) frost_task_rx: UnboundedReceiver<FrostNotificationMessage>,
    /// Frost Task Sender
    pub(crate) frost_task_tx: UnboundedSender<FrostNotificationMessage>,
    /// Frost Task Receiver
    pub(crate) pbft_task_rx: UnboundedReceiver<PbftNotificationMessage>,
    /// Frost Task Sender
    pub(crate) pbft_task_tx: UnboundedSender<PbftNotificationMessage>,
    /// utxo syncing engine
    pub(crate) utxo_sync: UTXOSyncEngine<EF, BF, DB, ToFrostMan>,
}
impl<EF, BF, DB, Engine: reth_node_api::EngineTypes, ToFrostMan>
    BlockProductionTask<EF, BF, DB, Engine, ToFrostMan>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    BF: BitcoindFactory + Clone + 'static,
    Engine: EngineTypes + 'static,
    ToFrostMan: ToFrostManager + Clone + 'static,
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        consensus: AuthorityConsensus,
        to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
        storage: Storage<EF, BF, DB>,
        btc_server: BtcServerExtendedClient,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        sk: secp256k1::SecretKey,
        epoch_manager: EpochManager<EF, BF, DB>,
        network_handle: NetworkHandle,
        payload_builder: PayloadBuilderHandle<EthEngineTypes>,
        frost_task_rx: UnboundedReceiver<FrostNotificationMessage>,
        frost_task_tx: UnboundedSender<FrostNotificationMessage>,
        pbft_task_rx: UnboundedReceiver<PbftNotificationMessage>,
        pbft_task_tx: UnboundedSender<PbftNotificationMessage>,
        utxo_sync: UTXOSyncEngine<EF, BF, DB, ToFrostMan>,
    ) -> Self {
        Self {
            consensus,
            storage,
            to_engine,
            pipe_line_events: None,
            btc_server,
            bitcoin_block_header,
            sk,
            epoch_manager,
            network_handle,
            payload_builder,
            frost_task_rx,
            frost_task_tx,
            pbft_task_rx,
            pbft_task_tx,
            utxo_sync,
        }
    }

    pub async fn start_task(&mut self) {
        loop {
            self.try_build_block().await;
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }

    /// Sets the pipeline events to listen on.
    pub fn set_pipeline_events(&mut self, events: UnboundedReceiverStream<PipelineEvent>) {
        self.pipe_line_events = Some(events);
    }
}

impl<EF, BF, DB, Engine: EngineTypes, ToFrostMan> std::fmt::Debug
    for BlockProductionTask<EF, BF, DB, Engine, ToFrostMan>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Authority Block Production Task").finish_non_exhaustive()
    }
}
