use crate::{
    comet_bft::abci::{ABCIClientBuilder, ABCIDriverMessage},
    frost_task::FrostTask,
    metrics::AuthorityMetrics,
    random_source_provider::RandomSource,
    snapshot_manager::SnapshotManager,
    wallet_state_sync::WalletStateSyncEngine,
    AuthorityConsensus, Storage,
};
use btcserverlib::extended_client::{
    BtcServerExtendedApi, BtcServerExtendedClient, GrpcClientFactory,
};
use comet_bft_rpc::HttpCometBFTRpcClientFactory;
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_chainspec::ChainSpec;
use reth_data_parser::{DataParser, SerializationType};
use reth_db::DatabaseEnv;
use reth_evm::execute::BlockExecutorProvider;
use reth_network::{
    frost::manager::{FrostConfig, ToFrostManager},
    NetworkHandle,
};
use reth_network_p2p::{BodiesClient, HeadersClient};
use reth_node_ethereum::{EthEngineTypes, EthEvmConfig};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::header_ext::HeaderExt;
use reth_provider::{
    BlockReaderIdExt, CanonStateNotification, CanonStateSubscriptions, ProviderFactory,
    StateProviderFactory,
};

use reth_tasks::TaskExecutor;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::RwLock;
use tracing::info;

pub(crate) type BitcoinCheckpoint = Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>;

/// Builder type for configuring the setup
#[allow(dead_code)]
pub struct AuthorityConsensusBuilder<EF, BF, DB, ToFrostMan, NetworkClient, Source> {
    consensus: AuthorityConsensus,
    storage: Storage<EF, BF, DB>,
    btc_server_factory: Option<GrpcClientFactory>,
    bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    network_handle: NetworkHandle,
    network_client: NetworkClient,
    frost_handle: Option<ToFrostMan>,
    task_executor: TaskExecutor,
    frost_config: Option<FrostConfig>,
    payload_builder: PayloadBuilderHandle<EthEngineTypes>,
    cometbft_rpc_factory: HttpCometBFTRpcClientFactory,
    random_source_provider: Source,
    metrics: Arc<AuthorityMetrics>,
    abci_driver_tx: tokio::sync::mpsc::Sender<ABCIDriverMessage>,
    provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
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
impl<EF, BF, DB, ToFrostMan, NetworkClient, Source>
    AuthorityConsensusBuilder<EF, BF, DB, ToFrostMan, NetworkClient, Source>
where
    ToFrostMan: ToFrostManager + Clone + 'static + Send,
    NetworkClient: BodiesClient + HeadersClient + Unpin + Clone + 'static,
    DB: BlockReaderIdExt + StateProviderFactory + Clone + 'static,
    NetworkClient: BodiesClient + HeadersClient + Unpin + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    BF: BitcoindFactory + Clone + Unpin + 'static,
    Source: RandomSource,
{
    /// Creates a new builder instance to configure all parts.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        chain_spec: Arc<ChainSpec>,
        client: DB,
        btc_server_factory: Option<GrpcClientFactory>,
        bitcoin_block_header: BitcoinCheckpoint,
        sk: secp256k1::SecretKey,
        network_handle: NetworkHandle,
        network_client: NetworkClient,
        frost_handle: Option<ToFrostMan>,
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
        random_source_provider: Source,
        abci_driver_tx: tokio::sync::mpsc::Sender<ABCIDriverMessage>,
        provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
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
            evm_config,
            chain_spec.clone(),
            bitcoind_factory,
            executor_factory,
            client.clone(),
        );

        Ok(Self {
            storage,
            consensus: AuthorityConsensus::new(chain_spec),
            btc_server_factory,
            bitcoin_block_header,
            network_handle,
            network_client,
            frost_handle,
            task_executor,
            frost_config,
            payload_builder,
            cometbft_rpc_factory,
            random_source_provider,
            metrics: Arc::new(AuthorityMetrics::default()),
            abci_driver_tx,
            provider_factory,
        })
    }

    /// Builds and returns the necessary components for the authority consensus, including the
    /// consensus itself, the client used to interact with the consensus, and the block
    /// production task.
    pub async fn build<BtcServerClient, Canon: CanonStateSubscriptions>(
        self,
        canon_notification_reciever: tokio::sync::broadcast::Receiver<CanonStateNotification>,
    ) -> (
        Option<FrostTask<EF, BF, DB, ToFrostMan, Source, BtcServerClient>>,
        Option<ABCIClientBuilder<EF, BF, DB>>,
        Option<SnapshotManager<EF, BF, DB>>,
    )
    where
        BtcServerClient: BtcServerExtendedApi + Clone + Send + Sync + 'static,
        BtcServerExtendedClient: Into<BtcServerClient>,
    {
        let Self {
            btc_server_factory,
            consensus,
            storage,
            bitcoin_block_header,
            network_handle,
            network_client: _,
            frost_handle,
            task_executor,
            frost_config,
            payload_builder: _,
            cometbft_rpc_factory,
            random_source_provider,
            metrics,
            abci_driver_tx,
            provider_factory,
        } = self;
        let is_fed_node = btc_server_factory.is_some();
        let chain_spec = storage.chain_spec.clone();
        let parser = DataParser::default().with_serialization_type(SerializationType::Json);

        let btc_server_client: Option<BtcServerClient> = async {
            if is_fed_node {
                Some(
                    btc_server_factory
                        .expect("btc_server_factory is available")
                        .build_and_connect()
                        .await
                        .expect("Failed to build and connect to btc server")
                        .into(),
                )
            } else {
                None
            }
        }
        .await;

        // TODO not used anywhere
        let _wallet_sync = {
            if let Some(btc_server) = &btc_server_client {
                let wallet_state_sync_engine = WalletStateSyncEngine::new(
                    storage.clone(),
                    btc_server.clone(),
                    frost_handle.clone().expect("Requires frost handle"),
                    parser.clone(),
                );
                Some(wallet_state_sync_engine)
            } else {
                None
            }
        };

        // create frost and block production tasks if btc_server is available:
        // only federation nodes will have btc_server
        let mut frost_task = None;
        if is_fed_node {
            // frost task
            let task = FrostTask::new(
                chain_spec.clone(),
                btc_server_client.clone().expect("btc_server is available"),
                network_handle.clone(),
                frost_handle.clone().expect("Requires frost handle"),
                frost_config.clone().expect("frost config exists"),
                storage.clone(),
                parser.clone(),
                random_source_provider,
                canon_notification_reciever,
                Arc::clone(&metrics),
            );

            frost_task = Some(task);
        }

        let (snapshot_manager_tx, snapshot_manager_rx) =
            tokio::sync::mpsc::channel::<ABCIDriverMessage>(100);

        // all nodes will have an abci client builder
        let abci_client_builder = Some(ABCIClientBuilder::new(
            storage.clone(),
            bitcoin_block_header,
            consensus.clone(),
            cometbft_rpc_factory.clone(),
            is_fed_node,
            Arc::clone(&metrics),
            task_executor.clone(),
            abci_driver_tx,
            provider_factory,
            snapshot_manager_tx,
        ));

        let snapshot_manager =
            Some(SnapshotManager::new(storage.clone(), parser.clone(), snapshot_manager_rx));

        (frost_task, abci_client_builder, snapshot_manager)
    }
}
