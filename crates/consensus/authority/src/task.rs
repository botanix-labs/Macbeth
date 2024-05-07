use crate::{
    epoch_manager::EpochManager, extended_client::BtcServerExtendedClient,
    frost_task::FrostNotificationMessage, Storage,
};
use reth_beacon_consensus::BeaconEngineMessage;

use reth_btc_wallet::bitcoind::BitcoindClient;
use reth_engine_primitives::EngineTypes;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_network::{frost::manager::FrostHandle, NetworkHandle};
use reth_node_api::ConfigureEvmEnv;
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::ChainSpec;
use reth_provider::{
    BlockReaderIdExt, CanonChainTracker, CanonStateNotificationSender, StateProviderFactory,
};
use reth_stages::PipelineEvent;
use reth_tasks::TaskExecutor;

use secp256k1::{All, Secp256k1};
use std::{collections::HashMap, sync::Arc};

use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    RwLock,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

pub struct BlockProductionTask<Client, EvmConfig, Engine: EngineTypes> {
    /// The configured chain spec
    pub(crate) chain_spec: Arc<ChainSpec>,
    /// The active epoch
    pub(crate) epoch_manager: EpochManager<Client>,
    /// Shared storage to insert new blocks
    pub(crate) storage: Storage<Client>,
    /// TODO: ideally this would just be a sender of hashes
    pub(crate) to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
    /// The pipeline events to listen on
    pub(crate) pipe_line_events: Option<UnboundedReceiverStream<PipelineEvent>>,
    /// BTC Server client
    pub(crate) btc_server: BtcServerExtendedClient,
    /// Recent bitcoin block headers
    pub(crate) bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    /// Recent bitcoin block tx ids
    pub(crate) bitcoin_block_tx_ids: Arc<RwLock<HashMap<u64, Vec<bitcoin::Txid>>>>,
    /// Bitcoind client
    pub(crate) bitcoind_client: BitcoindClient,
    /// Instance of secp
    pub(crate) secp: Secp256k1<All>,
    /// Key of authority
    pub(crate) sk: secp256k1::SecretKey,
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    /// Frost Handler
    pub(crate) frost_handle: FrostHandle,
    /// The type that defines how to configure the EVM.
    pub(crate) evm_config: EvmConfig,
    /// Task executor
    #[allow(dead_code)]
    task_executor: TaskExecutor,
    /// Ethereum Payload Builder
    pub(crate) payload_builder: PayloadBuilderHandle<Engine>,
    /// Frost Task Receiver
    pub(crate) frost_task_rx: UnboundedReceiver<FrostNotificationMessage>,
    /// Frost Task Sender
    pub(crate) frost_task_tx: UnboundedSender<FrostNotificationMessage>,
    /// Bitcoin Network
    pub(crate) btc_network: bitcoin::Network,
}
impl<Client, EvmConfig, Engine: EngineTypes>
    BlockProductionTask<Client, EvmConfig, Engine>
where
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    Engine: EngineTypes + 'static,
    EvmConfig:
        ConfigureEvmEnv + Clone + Unpin + Send + Sync + 'static + reth_node_api::ConfigureEvm,
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
        _canon_state_notification: CanonStateNotificationSender,
        storage: Storage<Client>,
        btc_server: BtcServerExtendedClient,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        bitcoin_block_tx_ids: Arc<RwLock<HashMap<u64, Vec<bitcoin::Txid>>>>,
        bitcoind_client: BitcoindClient,
        secp: Secp256k1<All>,
        sk: secp256k1::SecretKey,
        epoch_manager: EpochManager<Client>,
        network_handle: NetworkHandle,
        frost_handle: FrostHandle,
        task_executor: TaskExecutor,
        evm_config: EvmConfig,
        payload_builder: PayloadBuilderHandle<Engine>,
        frost_task_rx: UnboundedReceiver<FrostNotificationMessage>,
        frost_task_tx: UnboundedSender<FrostNotificationMessage>,
        btc_network: bitcoin::Network,
    ) -> Self {
        Self {
            chain_spec,
            storage,
            to_engine,
            pipe_line_events: None,
            btc_server,
            bitcoin_block_header,
            bitcoin_block_tx_ids,
            bitcoind_client,
            secp,
            sk,
            epoch_manager,
            network_handle,
            frost_handle,
            task_executor,
            evm_config,
            payload_builder,
            frost_task_rx,
            frost_task_tx,
            btc_network,
        }
    }

    pub async fn start_task(&mut self) {
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

impl<Client, EvmConfig: std::fmt::Debug, Engine: EngineTypes> std::fmt::Debug
    for BlockProductionTask<Client, EvmConfig, Engine>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Authority Block Production Task").finish_non_exhaustive()
    }
}
