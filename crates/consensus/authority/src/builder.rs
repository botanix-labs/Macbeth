use crate::{
    block_fetcher::BlockFetcherTask,
    comet_bft::abci::ABCIClientBuilder,
    compressor::Compressor,
    epoch_manager::EpochManager,
    frost_task::{FrostNotificationMessage, FrostTask},
    healthcheck_task::HealthcheckTask,
    sync::SyncController,
    utxo_sync::UTXOSyncEngine,
    AuthorityConsensus, Storage,
};
use btcserverlib::extended_client::GrpcClientFactory;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_interfaces::{
    blockchain_tree::BlockchainTreeEngine,
    p2p::{bodies::client::BodiesClient, headers::client::HeadersClient},
};
use reth_network::{
    frost::manager::{FrostConfig, ToFrostManager},
    message::NewBlockMessage,
    NetworkEvents, NetworkHandle,
};
use reth_node_api::EngineTypes;
use reth_node_ethereum::{EthEngineTypes, EthEvmConfig};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::{header_ext::HeaderExt, ChainSpec};
use reth_provider::{
    BlockReaderIdExt, CanonChainTracker, CanonStateNotificationSender, ExecutorFactory,
    StateProviderFactory,
};

use reth_tasks::TaskExecutor;
use secp256k1::{PublicKey, Secp256k1};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    RwLock,
};
use tracing::{error, info};

pub type BitcoinCheckpoint = Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>;

/// Builder type for confirguring the setup
pub struct AuthorityConsensusBuilder<EF, BF, DB, ToFrostMan, NetworkClient> {
    consensus: AuthorityConsensus,
    storage: Storage<EF, BF, DB>,
    to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
    canon_state_notification: CanonStateNotificationSender,
    btc_server_factory: Option<GrpcClientFactory>,
    bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    sk: secp256k1::SecretKey,
    epoch_manager: EpochManager<EF, BF, DB>,
    network_handle: NetworkHandle,
    network_client: NetworkClient,
    frost_handle: Option<ToFrostMan>,
    block_import_rx: UnboundedReceiver<NewBlockMessage>,
    task_executor: TaskExecutor,
    frost_config: Option<FrostConfig>,
    payload_builder: PayloadBuilderHandle<EthEngineTypes>,
    #[allow(dead_code)]
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
impl<EF, BF, DB, ToFrostMan, NetworkClient>
    AuthorityConsensusBuilder<EF, BF, DB, ToFrostMan, NetworkClient>
where
    ToFrostMan: ToFrostManager + Clone + 'static + Send,
    NetworkClient: BodiesClient + HeadersClient + Unpin + Clone + 'static,
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: ExecutorFactory + Clone + 'static,
    BF: BitcoindFactory + Clone + 'static,
{
    /// Creates a new builder instance to configure all parts.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        chain_spec: Arc<ChainSpec>,
        client: DB,
        to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
        canon_state_notification: CanonStateNotificationSender,
        btc_server_factory: Option<GrpcClientFactory>,
        bitcoin_block_header: BitcoinCheckpoint,
        sk: secp256k1::SecretKey,
        network_handle: NetworkHandle,
        network_client: NetworkClient,
        frost_handle: Option<ToFrostMan>,
        block_import_rx: UnboundedReceiver<NewBlockMessage>,
        task_executor: TaskExecutor,
        frost_config: Option<FrostConfig>,
        payload_builder: PayloadBuilderHandle<EthEngineTypes>,
        btc_network: bitcoin::Network,
        genesis_authorities: Vec<secp256k1::PublicKey>,
        authority_socket_addresses: Vec<SocketAddr>,
        executor_factory: EF,
        bitcoind_factory: BF,
        evm_config: EthEvmConfig,
    ) -> Result<Self, AuthorityConsensusBuilderError> {
        // only a federation node has a btc_server
        let is_fed_node = btc_server_factory.is_some();

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

        let agg_pk = {
            if latest_header.number > 0 {
                Some(
                    latest_header
                        .get_aggregate_public_key()
                        .expect("latest header is greater than genesis"),
                )
            } else {
                None
            }
        };
        info!("Aggregate public key: {:?}", agg_pk);

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

        // TODO REMOVE LATER
        let mut rng = &mut secp256k1::rand::thread_rng();
        let agg_pk = secp256k1::generate_keypair(&mut rng).1;
        let pk = sk.public_key(secp256k1::SECP256K1);

        // Try to instantiate storage
        let storage = Storage::new(
            genesis_authorities,
            authorities,
            signer_index.expect("valid index"),
            pk,
            btc_network,
            // Aggregate pk to be filled out by the dkg state machine if we are still on genesis
            // block
            Some(agg_pk),
            authority_socket_addresses,
            evm_config.clone(),
            chain_spec.clone(),
            bitcoind_factory,
            executor_factory,
            client.clone(),
        );

        // Instantiate epoch manager
        let epoch_manager = EpochManager::new(storage.clone());

        Ok(Self {
            storage,
            consensus: AuthorityConsensus::new(chain_spec),
            to_engine,
            canon_state_notification,
            btc_server_factory,
            bitcoin_block_header,
            sk,
            epoch_manager,
            network_handle,
            network_client,
            frost_handle,
            block_import_rx,
            task_executor,
            frost_config,
            payload_builder,
            btc_network,
        })
    }

    /// Builds and returns the necessary components for the authority consensus, including the
    /// consensus itself, the client used to interact with the consensus, and the block
    /// production task.
    pub async fn build(
        self,
    ) -> (
        AuthorityConsensus,
        BlockFetcherTask<EF, BF, DB, NetworkClient, ToFrostMan>,
        Option<FrostTask<EF, BF, DB, ToFrostMan>>,
        SyncController,
        Option<HealthcheckTask<EF, BF, DB, ToFrostMan>>,
        Option<ABCIClientBuilder<EF, BF, DB>>,
    ) {
        let Self {
            btc_server_factory,
            consensus,
            storage,
            to_engine,
            canon_state_notification,
            bitcoin_block_header,
            sk,
            epoch_manager,
            network_handle,
            network_client,
            frost_handle,
            block_import_rx,
            task_executor,
            frost_config,
            payload_builder,
            btc_network: _,
        } = self;
        let is_fed_node = btc_server_factory.is_some();
        let guard = storage.read().await;
        let executor_factory = guard.executor_factory.clone();
        let chain_spec = guard.chain_spec.clone();
        drop(guard);
        let compressor = Compressor::new();

        let btc_server_client = async {
            if is_fed_node {
                Some(
                    btc_server_factory
                        .expect("btc_server_factory is available")
                        .build_and_connect()
                        .await
                        .expect("Failed to build and connect to btc server"),
                )
            } else {
                None
            }
        }
        .await;

        let utxo_sync = {
            if let Some(btc_server) = &btc_server_client {
                let utxo_set_sync_engine = UTXOSyncEngine::new(
                    storage.clone(),
                    btc_server.clone(),
                    frost_handle.clone().expect("Requires frost handle"),
                    compressor.clone(),
                );
                Some(utxo_set_sync_engine)
            } else {
                None
            }
        };

        let sync_task = SyncController::new(
            network_handle.clone().event_listener(),
            *network_handle.peer_id(),
            to_engine.clone(),
        );

        let block_fetcher_task = BlockFetcherTask::new(
            consensus.clone(),
            block_import_rx,
            to_engine.clone(),
            canon_state_notification.clone(),
            btc_server_client.clone(),
            storage.clone(),
            bitcoin_block_header.clone(),
            network_client.clone(),
            network_handle.clone(),
            utxo_sync.clone(),
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
        let mut healthcheck_task = None;
        let mut abci_client_builder = None;
        if is_fed_node {
            let task = HealthcheckTask::new(
                network_handle.clone(),
                frost_handle.clone().expect("Requires frost handle"),
                storage.clone(),
                task_executor.clone(),
            );
            healthcheck_task = Some(task);
            // frost task
            let task = FrostTask::new(
                chain_spec.clone(),
                btc_server_client.clone().expect("btc_server is available"),
                network_handle.clone(),
                frost_handle.clone().expect("Requires frost handle"),
                frost_config.clone().expect("frost config exists"),
                storage.clone(),
                frost_task_notifications1_rx,
                frost_task_notifications2_tx,
                task_executor.clone(),
                compressor,
            );

            frost_task = Some(task);
            abci_client_builder = Some(ABCIClientBuilder::new(
                storage.clone(),
                bitcoin_block_header.clone(),
                network_handle.clone(),
                btc_server_client.expect("to be defined").clone(),
                consensus.clone(),
                to_engine.clone(),
            ));
        }

        (
            consensus,
            block_fetcher_task,
            frost_task,
            sync_task,
            healthcheck_task,
            abci_client_builder,
        )
    }
}
