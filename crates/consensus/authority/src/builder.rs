use crate::{
    comet_bft::abci::ABCIClientBuilder,
    compressor::Compressor,
    frost_task::{FrostNotificationMessage, FrostTask},
    healthcheck_task::HealthcheckTask,
    sync::SyncController,
    utxo_sync::UTXOSyncEngine,
    AuthorityConsensus, Storage,
};
use btcserverlib::extended_client::GrpcClientFactory;
use comet_bft_rpc::HttpCometBFTRpcClientFactory;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_blockchain_tree_api::BlockchainTreeEngine;
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_chainspec::ChainSpec;
use reth_evm::execute::BlockExecutorProvider;
use reth_network::{
    frost::manager::{FrostConfig, ToFrostManager},
    message::NewBlockMessageWithPeerId,
    NetworkEventListenerProvider, NetworkHandle,
};
use reth_network_p2p::{BodiesClient, HeadersClient};
use reth_node_ethereum::{EthEngineTypes, EthEvmConfig};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::header_ext::HeaderExt;
use reth_provider::{
    BlockReaderIdExt, CanonChainTracker, CanonStateNotificationSender, StateProviderFactory,
};

use reth_tasks::TaskExecutor;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    RwLock,
};
use tracing::info;

pub(crate) type BitcoinCheckpoint = Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>;

/// Builder type for confirguring the setup
pub struct AuthorityConsensusBuilder<EF, BF, DB, ToFrostMan, NetworkClient> {
    consensus: AuthorityConsensus,
    storage: Storage<EF, BF, DB>,
    to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
    canon_state_notification: CanonStateNotificationSender,
    btc_server_factory: Option<GrpcClientFactory>,
    bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    sk: secp256k1::SecretKey,
    network_handle: NetworkHandle,
    network_client: NetworkClient,
    frost_handle: Option<ToFrostMan>,
    block_import_rx: UnboundedReceiver<NewBlockMessageWithPeerId>,
    task_executor: TaskExecutor,
    frost_config: Option<FrostConfig>,
    payload_builder: PayloadBuilderHandle<EthEngineTypes>,
    cometbft_rpc_factory: HttpCometBFTRpcClientFactory,
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
    NetworkClient: BodiesClient + HeadersClient + Unpin + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    BF: BitcoindFactory + Clone + Unpin + 'static,
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
        block_import_rx: UnboundedReceiver<NewBlockMessageWithPeerId>,
        task_executor: TaskExecutor,
        frost_config: Option<FrostConfig>,
        payload_builder: PayloadBuilderHandle<EthEngineTypes>,
        btc_network: bitcoin::Network,
        genesis_authorities: Vec<secp256k1::PublicKey>,
        authority_socket_addresses: Vec<SocketAddr>,
        executor_factory: EF,
        bitcoind_factory: BF,
        evm_config: EthEvmConfig,
        cometbft_rpc_factory: HttpCometBFTRpcClientFactory,
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
        let mut signer_index = Some(genesis_authorities.len() + 1);
        // only a federation node has a btc_server
        if is_fed_node {
            signer_index =
                genesis_authorities.iter().position(|a| *a == sk.public_key(secp256k1::SECP256K1));

            if signer_index.is_none() {
                return Err(AuthorityConsensusBuilderError::FailedToFindSignerIndex);
            }
        }
        let pk = sk.public_key(secp256k1::SECP256K1);

        // Try to instantiate storage
        let storage = Storage::new(
            genesis_authorities,
            signer_index.expect("valid index"),
            pk,
            btc_network,
            // Aggregate pk to be filled out by the dkg state machine if we are still on genesis
            // block
            agg_pk,
            authority_socket_addresses,
            evm_config.clone(),
            chain_spec.clone(),
            bitcoind_factory,
            executor_factory,
            client.clone(),
        );

        Ok(Self {
            storage,
            consensus: AuthorityConsensus::new(chain_spec),
            to_engine,
            canon_state_notification,
            btc_server_factory,
            bitcoin_block_header,
            sk,
            network_handle,
            network_client,
            frost_handle,
            block_import_rx,
            task_executor,
            frost_config,
            payload_builder,
            cometbft_rpc_factory,
        })
    }

    /// Builds and returns the necessary components for the authority consensus, including the
    /// consensus itself, the client used to interact with the consensus, and the block
    /// production task.
    pub async fn build(
        self,
    ) -> (
        AuthorityConsensus,
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
            canon_state_notification: _,
            bitcoin_block_header,
            sk: _,
            network_handle,
            network_client: _,
            frost_handle,
            block_import_rx,
            task_executor,
            frost_config,
            payload_builder: _,
            cometbft_rpc_factory,
        } = self;
        let is_fed_node = btc_server_factory.is_some();
        let _executor_factory = storage.executor_factory.clone();
        let chain_spec = storage.chain_spec.clone();
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

        let _utxo_sync = {
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

        // Set up frost notification message queue
        // these are two mpsc channels that are used to communicate between the frost task and the
        // block production task
        let (_frost_task_notifications1_tx, frost_task_notifications1_rx) =
            tokio::sync::mpsc::unbounded_channel::<FrostNotificationMessage>();
        let (frost_task_notifications2_tx, _frost_task_notifications2_rx) =
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
        }

        // all nodes will have an abci client builder
        abci_client_builder = Some(ABCIClientBuilder::new(
            storage.clone(),
            bitcoin_block_header.clone(),
            network_handle.clone(),
            btc_server_client,
            consensus.clone(),
            to_engine.clone(),
            cometbft_rpc_factory.clone(),
        ));

        (consensus, frost_task, sync_task, healthcheck_task, abci_client_builder)
    }
}
