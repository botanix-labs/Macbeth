use crate::{
    block_fetcher::BlockFetcherTask,
    epoch_manager::EpochManager,
    extended_client::BtcServerExtendedClient,
    frost_task::{FrostNotificationMessage, FrostTask},
    healthcheck_task::HealthcheckTask,
    pbft_task::{PbftNotificationMessage, PbftTask},
    task::BlockProductionTask,
    AuthorityConsensus, Storage,
};

use crate::sync::SyncController;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_btc_wallet::bitcoind::{BitcoindClient, BitcoindConfig};
use reth_interfaces::{
    blockchain_tree::BlockchainTreeEngine,
    p2p::{bodies::client::BodiesClient, headers::client::HeadersClient},
};
use reth_network::{
    frost::manager::{FrostConfig, ToFrostManager},
    message::NewBlockMessage,
    NetworkEvents, NetworkHandle,
};
use reth_node_api::{ConfigureEvmEnv, EngineTypes};
use reth_node_ethereum::EthEngineTypes;
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::{header_ext::HeaderExt, ChainSpec};
use reth_provider::{
    BlockReaderIdExt, CanonChainTracker, CanonStateNotificationSender, StateProviderFactory,
};
use reth_tasks::TaskExecutor;
use std::sync::Arc;
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    RwLock,
};
use tracing::error;

/// Builder type for confirguring the setup
pub struct AuthorityConsensusBuilder<
    Client,
    EvmConfig,
    Engine: EngineTypes,
    ToFrostMan,
    NetworkClient,
> {
    client: Client,
    consensus: AuthorityConsensus,
    storage: Storage,
    to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
    canon_state_notification: CanonStateNotificationSender,
    btc_server: Option<BtcServerExtendedClient>,
    bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    bitcoind_config: BitcoindConfig,
    sk: secp256k1::SecretKey,
    #[allow(dead_code)]
    epoch_manager: EpochManager<Client>,
    network_handle: NetworkHandle,
    network_client: NetworkClient,
    frost_handle: Option<ToFrostMan>,
    block_import_rx: UnboundedReceiver<NewBlockMessage>,
    task_executor: TaskExecutor,
    /// The type that defines how to configure the EVM.
    evm_config: EvmConfig,
    frost_config: Option<FrostConfig>,
    payload_builder: PayloadBuilderHandle<EthEngineTypes>,
    btc_network: bitcoin::Network,
}

/// Errors that can occur when building an authority consensus.
#[derive(Debug)]
pub enum AuthorityConsensusBuilderError {
    InvalidStorage,
    FailedToRecoverAuthorityList,
    FailedToFindSignerIndex,
    FailedToRetrieveEopchHeader,
}

// ===== impl AuthorityConsensusBuilder =====
impl<Client, EvmConfig, Engine, ToFrostMan, NetworkClient>
    AuthorityConsensusBuilder<Client, EvmConfig, Engine, ToFrostMan, NetworkClient>
where
    ToFrostMan: ToFrostManager + Clone + 'static + Send,
    Engine: EngineTypes + 'static,
    EvmConfig:
        ConfigureEvmEnv + Clone + Unpin + Send + Sync + 'static + reth_node_api::ConfigureEvm,
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    NetworkClient: BodiesClient + HeadersClient + Unpin + Clone + 'static,
{
    /// Creates a new builder instance to configure all parts.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        chain_spec: Arc<ChainSpec>,
        client: Client,
        to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
        canon_state_notification: CanonStateNotificationSender,
        btc_server: Option<BtcServerExtendedClient>,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        bitcoind_config: BitcoindConfig,
        // TODO (armins) This should be Arc protected
        sk: secp256k1::SecretKey,
        network_handle: NetworkHandle,
        network_client: NetworkClient,
        frost_handle: Option<ToFrostMan>,
        block_import_rx: UnboundedReceiver<NewBlockMessage>,
        task_executor: TaskExecutor,
        evm_config: EvmConfig,
        frost_config: Option<FrostConfig>,
        payload_builder: PayloadBuilderHandle<EthEngineTypes>,
        btc_network: bitcoin::Network,
        genesis_authorities: Vec<secp256k1::PublicKey>,
    ) -> Result<Self, AuthorityConsensusBuilderError> {
        // only a federation node has a btc_server
        let is_fed_node = btc_server.is_some();

        let mut latest_header = client
            .latest_header()
            .ok()
            .flatten()
            .unwrap_or_else(|| chain_spec.sealed_genesis_header());
        let mut headers = vec![latest_header.clone()];

        while !latest_header.header().is_poa_epoch() {
            let parent_hash = latest_header.parent_hash;

            if let Some(new_header) = client.header(&parent_hash).ok().flatten() {
                let old_latest_header =
                    std::mem::replace(&mut latest_header, new_header.seal_slow());
                headers.push(old_latest_header);
            } else {
                return Err(AuthorityConsensusBuilderError::FailedToRetrieveEopchHeader);
            }
        }

        // Latest epoch header is the last header in the vector
        // This header should include the authority list which is validated by consensus
        let authorities = latest_header
            .get_authority_list()
            .map_err(|e| {
                error!("Failed to retrieve authority list: {:?}", e);
                AuthorityConsensusBuilderError::FailedToRecoverAuthorityList
            })?
            .expect("authority signer list in epoch block");

        // authority length represents a non federation node since it would be out of bounds
        // this prevents the node from signing blocks although there are other checks to stop this
        // as well
        let mut signer_index = Some(authorities.len() + 1);
        // only a federation node has a btc_server
        if is_fed_node {
            signer_index =
                authorities.iter().position(|a| *a == sk.public_key(secp256k1::SECP256K1));

            if signer_index.is_none() {
                return Err(AuthorityConsensusBuilderError::FailedToFindSignerIndex);
            }
        }

        let pk = sk.public_key(&secp256k1::SECP256K1);

        // Try to instantiate storage
        let storage = Storage::try_new(
            &mut headers,
            genesis_authorities,
            authorities,
            signer_index.expect("valid index"),
            pk,
        )
        .map_err(|e| {
            error!("Failed to instantiate storage: {:?}", e);
            AuthorityConsensusBuilderError::InvalidStorage
        })?;

        // Instantiate epoch manager
        let epoch_manager = EpochManager::<Client>::new(storage.clone(), client.clone());

        Ok(Self {
            storage,
            client,
            consensus: AuthorityConsensus::new(chain_spec),
            to_engine,
            canon_state_notification,
            btc_server,
            bitcoin_block_header,
            bitcoind_config,
            sk,
            epoch_manager,
            network_handle,
            network_client,
            frost_handle,
            block_import_rx,
            task_executor,
            evm_config,
            frost_config,
            payload_builder,
            btc_network,
        })
    }

    #[track_caller]
    /// Builds and returns the necessary components for the authority consensus, including the
    /// consensus itself, the client used to interact with the consensus, and the block
    /// production task.
    pub fn build(
        self,
    ) -> (
        AuthorityConsensus,
        Option<BlockProductionTask<Client, EvmConfig, Engine, ToFrostMan>>,
        BlockFetcherTask<Client, EvmConfig, Engine, NetworkClient>,
        Option<FrostTask<Client, ToFrostMan>>,
        SyncController<Engine>,
        Option<PbftTask<Client, ToFrostMan, NetworkClient>>,
        HealthcheckTask<ToFrostMan>,
    ) {
        let Self {
            btc_server,
            client,
            consensus,
            storage,
            to_engine,
            canon_state_notification,
            bitcoin_block_header,
            bitcoind_config,
            sk,
            epoch_manager,
            network_handle,
            network_client,
            frost_handle,
            block_import_rx,
            task_executor,
            evm_config,
            frost_config,
            payload_builder,
            btc_network,
        } = self;
        let is_fed_node = btc_server.is_some();

        let sync_task = SyncController::new(
            network_handle.clone().event_listener(),
            *network_handle.peer_id(),
            to_engine.clone(),
        );

        let block_fetcher_task = crate::block_fetcher::BlockFetcherTask::new(
            consensus.clone(),
            block_import_rx,
            to_engine.clone(),
            canon_state_notification.clone(),
            btc_server.clone(),
            storage.clone(),
            bitcoin_block_header.clone(),
            evm_config.clone(),
            btc_network,
            network_client.clone(),
            network_handle.clone(),
            client.clone(),
        );

        let healthcheck_task = HealthcheckTask::new(
            network_handle.clone(),
            frost_handle.clone().expect("Requires frost handle"),
            storage.clone(),
            task_executor.clone(),
        );

        // Set up frost notification message queue
        // these are two mpsc channels that are used to communicate between the frost task and the
        // block production task
        let (frost_task_notifications1_tx, frost_task_notifications1_rx) =
            tokio::sync::mpsc::unbounded_channel::<FrostNotificationMessage>();
        let (frost_task_notifications2_tx, frost_task_notifications2_rx) =
            tokio::sync::mpsc::unbounded_channel::<FrostNotificationMessage>();
        // create frost and block production tasks if btc_server is available:
        // only federation nodes will have btc_server
        let mut frost_task = None;
        let mut block_production_task = None;
        let mut pbft_task = None;
        if is_fed_node {
            // frost task
            let task = FrostTask::new(
                btc_server.clone().expect("btc_server is available"),
                network_handle.clone(),
                frost_handle.clone().expect("Requires frost handle"),
                epoch_manager.clone(),
                frost_config.clone().expect("frost config exists"),
                storage.clone(),
                frost_task_notifications1_rx,
                frost_task_notifications2_tx,
                task_executor.clone(),
            );

            frost_task = Some(task);

            // Set up pbft notification message queue
            // these are two mpsc channels that are used to communicate between the pbft task and
            // the block production task
            let (pbft_task_notifications1_tx, pbft_task_notifications1_rx) =
                tokio::sync::mpsc::unbounded_channel::<PbftNotificationMessage>();
            let (pbft_task_notifications2_tx, pbft_task_notifications2_rx) =
                tokio::sync::mpsc::unbounded_channel::<PbftNotificationMessage>();

            let pbft = PbftTask::new(
                client.clone(),
                frost_handle.clone().expect("Requires frost handle"),
                frost_config.expect("valid frost config"),
                sk,
                pbft_task_notifications1_rx,
                pbft_task_notifications2_tx,
                task_executor.clone(),
                network_client,
                network_handle.clone(),
            );
            pbft_task = Some(pbft);

            let _bitcoind_client =
                BitcoindClient::new(bitcoind_config).expect("Invalid Bitcoind client");
            let block_production = BlockProductionTask::new(
                consensus.clone(),
                to_engine,
                canon_state_notification,
                storage,
                btc_server.clone().expect("btc_server is available"),
                bitcoin_block_header,
                sk,
                epoch_manager,
                network_handle,
                frost_handle.expect("Requires frost handle"),
                task_executor,
                evm_config.clone(),
                payload_builder,
                frost_task_notifications2_rx,
                frost_task_notifications1_tx,
                pbft_task_notifications2_rx,
                pbft_task_notifications1_tx,
                btc_network,
                client.clone(),
            );

            block_production_task = Some(block_production);
        }

        (
            consensus,
            block_production_task,
            block_fetcher_task,
            frost_task,
            sync_task,
            pbft_task,
            healthcheck_task,
        )
    }
}
