//! Main node command
//!
//! Starts the client
use crate::{
    args::{
        get_secret_key,
        utils::{chain_help, genesis_value_parser, parse_socket_address, SUPPORTED_CHAINS},
        DatabaseArgs, DebugArgs, NetworkArgs, PayloadBuilderArgs, PruningArgs, RpcServerArgs,
        TxPoolArgs,
    },
    cli::{
        components::RethNodeComponentsImpl,
        config::RethRpcConfig,
        ext::{RethCliExt, RethNodeCommandConfig},
    },
    dirs::{ChainPath, DataDirPath, MaybePlatformPath},
    init::init_genesis,
    node::cl_events::ConsensusLayerHealthEvents,
    prometheus_exporter,
    runner::CliContext,
    utils::get_single_header,
    version::SHORT_VERSION,
};
use clap::{value_parser, Parser};
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::{future::Either, pin_mut, stream, stream_select, StreamExt};
use metrics_exporter_prometheus::PrometheusHandle;
use reth_authority_consensus::{AuthorityConsensus, AuthorityConsensusBuilder};

use reth_beacon_consensus::{
    hooks::{EngineHooks, PruneHook},
    BeaconConsensusEngine, MIN_BLOCKS_FOR_PIPELINE_RUN,
};
use reth_blockchain_tree::{
    config::BlockchainTreeConfig, externals::TreeExternals, BlockchainTree, ShareableBlockchainTree,
};
use reth_config::{
    config::{PruneConfig, StageConfig},
    Config,
};
use reth_consensus_common::utils;
use reth_db::{database::Database, init_db, DatabaseEnv};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_interfaces::{
    consensus::Consensus,
    p2p::{
        bodies::{client::BodiesClient, downloader::BodyDownloader},
        headers::{client::HeadersClient, downloader::HeaderDownloader},
    },
    RethResult,
};
use reth_network::{
    config::NetworkMode,
    import::{BlockImport, ProofOfAuthorityBlockImport},
    NetworkBuilder, NetworkConfig, NetworkEvents, NetworkHandle, NetworkManager,
};
use reth_network_api::{NetworkInfo, PeersInfo};
use reth_primitives::{
    constants::eip4844::{LoadKzgSettingsError, MAINNET_KZG_TRUSTED_SETUP},
    kzg::KzgSettings,
    stage::StageId,
    BlockHashOrNumber, BlockNumber, ChainSpec, Head, SealedHeader, B256,
};
use reth_provider::{
    providers::BlockchainProvider, BlockHashReader, BlockReader, CanonStateSubscriptions,
    HeaderProvider, HeaderSyncMode, ProviderFactory, StageCheckpointReader,
};
use reth_prune::{segments::SegmentSet, Pruner};
use reth_revm::EvmProcessorFactory;
use reth_revm_inspectors::stack::Hook;
use reth_rpc_engine_api::EngineApi;
use reth_snapshot::HighestSnapshotsTracker;
use reth_stages::{
    prelude::*,
    stages::{
        AccountHashingStage, ExecutionStage, ExecutionStageThresholds, IndexAccountHistoryStage,
        IndexStorageHistoryStage, MerkleStage, SenderRecoveryStage, StorageHashingStage,
        TotalDifficultyStage, TransactionLookupStage,
    },
};
use reth_tasks::TaskExecutor;
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, TransactionPool, TransactionValidationTaskExecutor,
};
use secp256k1::SecretKey;
use std::{
    net::{SocketAddr, SocketAddrV4},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{mpsc::unbounded_channel, oneshot, watch, RwLock};
use tracing::*;

use client::BtcServerClient;
use reth_btc_wallet::block_source::{BlockSource, MempoolSpace};

use rsntp::AsyncSntpClient;
pub mod cl_events;
pub mod events;

/// Start the node
#[derive(Debug, Parser)]
pub struct PoaNodeCommand<Ext: RethCliExt = ()> {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
    pub datadir: MaybePlatformPath<DataDirPath>,

    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    pub config: Option<PathBuf>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        default_value_if("dev", "true", "dev"),
        value_parser = genesis_value_parser,
        required = false,
    )]
    pub chain: Arc<ChainSpec>,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long, value_name = "SOCKET", value_parser = parse_socket_address, help_heading = "Metrics")]
    pub metrics: Option<SocketAddr>,

    /// Add a new instance of a node.
    ///
    /// Configures the ports of the node to avoid conflicts with the defaults.
    /// This is useful for running multiple nodes on the same machine.
    ///
    /// Max number of instances is 200. It is chosen in a way so that it's not possible to have
    /// port numbers that conflict with each other.
    ///
    /// Changes to the following port numbers:
    /// - DISCOVERY_PORT: default + `instance` - 1
    /// - AUTH_PORT: default + `instance` * 100 - 100
    /// - HTTP_RPC_PORT: default - `instance` + 1
    /// - WS_RPC_PORT: default + `instance` * 2 - 2
    #[arg(long, value_name = "INSTANCE", global = true, default_value_t = 1, value_parser = value_parser!(u16).range(..=200))]
    pub instance: u16,

    /// Overrides the KZG trusted setup by reading from the supplied file.
    #[arg(long, value_name = "PATH")]
    pub trusted_setup_file: Option<PathBuf>,

    /// All networking related arguments
    #[clap(flatten)]
    pub network: NetworkArgs,

    /// All rpc related arguments
    #[clap(flatten)]
    pub rpc: RpcServerArgs,

    /// All txpool related arguments with --txpool prefix
    #[clap(flatten)]
    pub txpool: TxPoolArgs,

    /// All payload builder related arguments
    #[clap(flatten)]
    pub builder: PayloadBuilderArgs,

    /// All debug related arguments with --debug prefix
    #[clap(flatten)]
    pub debug: DebugArgs,

    /// All database related arguments
    #[clap(flatten)]
    pub db: DatabaseArgs,

    /// All pruning related arguments
    #[clap(flatten)]
    pub pruning: PruningArgs,

    /// Additional cli arguments
    #[clap(flatten)]
    #[clap(next_help_heading = "Extension")]
    pub ext: Ext::Node,
}

impl<Ext: RethCliExt> PoaNodeCommand<Ext> {
    /// Replaces the extension of the node command
    pub fn with_ext<E: RethCliExt>(self, ext: E::Node) -> PoaNodeCommand<E> {
        let Self {
            datadir,
            config,
            chain,
            metrics,
            trusted_setup_file,
            instance,
            network,
            rpc,
            txpool,
            builder,
            debug,
            db,
            pruning,
            ..
        } = self;
        PoaNodeCommand {
            datadir,
            config,
            chain,
            metrics,
            instance,
            trusted_setup_file,
            network,
            rpc,
            txpool,
            builder,
            debug,
            db,
            pruning,
            ext,
        }
    }

    // get unix timsestamp in seconds from ntp server
    async fn ntp_unix_timestamp(ntp_server: &str) -> eyre::Result<u64> {
        // create NTP client
        let client = AsyncSntpClient::new();

        // sync with NTP server
        match client.synchronize(ntp_server).await {
            Ok(sync_result) => match sync_result.datetime().unix_timestamp() {
                Ok(duration) => Ok(duration.as_secs()),
                Err(err) => {
                    error!("Failed to get unix timestamp from NTP response: {}", err);
                    Err(err.into())
                }
            },
            Err(err) => {
                error!("Failed to sync with NTP server: {}", err);
                Err(err.into())
            }
        }
    }

    /// Execute `node` command
    pub async fn execute(mut self, ctx: CliContext) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        // Raise the fd limit of the process.
        // Does not do anything on windows.
        raise_fd_limit();

        // async task that checks system clock is in sync with NTP server
        ctx.task_executor.spawn_critical(
            "async system clock sync with ntp task",
            Box::pin(async {
                let sleep_sec = tokio::time::Duration::from_secs(15);
                let acceptable_drift_sec = 1;
                loop {
                    // TODO (scott) pass in ntp url as arg
                    match Self::ntp_unix_timestamp("time.cloudflare.com").await {
                        Ok(ntp_timestamp) => {
                            let system_timestamp = utils::unix_timestamp();
                            if (ntp_timestamp as i64 - system_timestamp as i64).abs() > acceptable_drift_sec {
                                error!("System clock is not in sync with NTP server. System timestamp: {}, NTP timestamp: {}", system_timestamp, ntp_timestamp);
                            } else {
                                info!("System clock is in sync with NTP server. System timestamp: {}, NTP timestamp: {}", system_timestamp, ntp_timestamp);
                            }
                        }
                        Err(err) => {
                            error!("NTP sync failed: {}", err);
                        }
                    }
                    tokio::time::sleep(sleep_sec).await;
                }
            }),
        );

        // get config
        let config = self.load_config()?;

        // Set up consensus
        let authority_consensus: Arc<dyn Consensus> =
            Arc::new(AuthorityConsensus::new(self.chain.clone()));
        // Connect to btc signining server
        let btc_server_client: BtcServerClient<tonic::transport::Channel> =
            BtcServerClient::connect(self.rpc.btc_server.clone())
                .await
                .expect("connect to btc_server");
        info!(target: "reth::cli", "Btc server connected");

        let bitcoin_block_headers: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>> =
            Arc::new(RwLock::new(None));
        let bitcoin_block_headers_clone = bitcoin_block_headers.clone();

        let block_source = MempoolSpace::new(self.rpc.btc_block_source.to_string().clone());

        ctx.task_executor.spawn_critical(
            "async bitcoin block header task",
            Box::pin(async move {
                let sleep_ms = tokio::time::Duration::from_millis(5000);
                let mut tip = 0u32;
                loop {
                    let mut header_write = bitcoin_block_headers.write().await;
                    let current_tip = match block_source.get_tip().await {
                        Ok(current_tip) => current_tip,
                        Err(_) => {
                            drop(header_write);
                            error!(target: "reth::cli", "Failed to fetch the tip. Retrying...");
                            tokio::time::sleep(sleep_ms).await;
                            continue;
                        }
                    };
                    if current_tip != tip {
                        info!("Async bitcoin worker tip mismatch");
                        let block_hash = match block_source.get_block_hash(current_tip).await {
                            Ok(block_hash) => block_hash,
                            Err(_) => {
                                drop(header_write);
                                error!(target: "reth::cli", "Failed to fetch a block hash. Retrying...");
                                tokio::time::sleep(sleep_ms).await;
                                continue;
                            }
                        };
                        let block_header = match block_source.get_block_header(block_hash).await {
                            Ok(block_header) => block_header,
                            Err(_) => {
                                drop(header_write);
                                error!(target: "reth::cli", "Failed to fetch a block header. Retrying...");
                                tokio::time::sleep(sleep_ms).await;
                                continue;
                            }
                        };
                        // TODO (armins) in v1 we will need the nth deep block header not tip
                        *header_write = Some((block_header, current_tip));
                        drop(header_write);
                        tip = current_tip;
                    }
                    tokio::time::sleep(sleep_ms).await;
                }
            }),
        );
        info!(target: "reth::cli", "Spawned async bitcoin block header task");

        let prometheus_handle =
            if self.metrics.is_some() { Some(self.install_prometheus_recorder()?) } else { None };

        // always store reth.toml in the data dir, not the chain specific data dir
        info!(target: "reth::cli", path = ?self.config_path(), "Configuration loaded");

        let db_path = self.data_dir().db_path();

        info!(target: "reth::cli", path = ?db_path, "Opening database");
        let db = Arc::new(init_db(&db_path, self.db.log_level)?.with_metrics());
        info!(target: "reth::cli", "Database opened");

        let mut provider_factory = ProviderFactory::new(Arc::clone(&db), Arc::clone(&self.chain));

        // configure snapshotter
        let snapshotter = reth_snapshot::Snapshotter::new(
            provider_factory.clone(),
            self.data_dir().snapshots_path(),
            self.chain.snapshot_block_interval,
        )?;

        provider_factory = provider_factory.with_snapshots(
            self.data_dir().snapshots_path(),
            snapshotter.highest_snapshot_receiver(),
        );

        self.start_metrics_endpoint(prometheus_handle, Arc::clone(&db)).await?;

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");

        let genesis_hash = init_genesis(Arc::clone(&db), self.chain.clone())?;

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        ctx.task_executor.spawn_critical("stages metrics listener task", sync_metrics_listener);

        let prune_config =
            self.pruning.prune_config(Arc::clone(&self.chain))?.or(config.prune.clone());

        // configure blockchain tree
        let tree_externals = TreeExternals::new(
            provider_factory.clone(),
            Arc::clone(&authority_consensus),
            EvmProcessorFactory::new(self.chain.clone()),
        );
        let tree_config = BlockchainTreeConfig::default();
        let tree = BlockchainTree::new(
            tree_externals,
            tree_config,
            prune_config.clone().map(|config| config.segments),
        )?
        .with_sync_metrics_tx(sync_metrics_tx.clone());
        let canon_state_notification_sender = tree.canon_state_notification_sender();
        let blockchain_tree = ShareableBlockchainTree::new(tree);
        debug!(target: "reth::cli", "configured blockchain tree");

        // fetch the head block from the database
        let head = self.lookup_head(Arc::clone(&db)).wrap_err("the head block is missing")?;

        // setup the blockchain provider
        let blockchain_db =
            BlockchainProvider::new(provider_factory.clone(), blockchain_tree.clone())?;
        let blob_store = InMemoryBlobStore::default();
        let validator = TransactionValidationTaskExecutor::eth_builder(Arc::clone(&self.chain))
            .with_head_timestamp(head.timestamp)
            .kzg_settings(self.kzg_settings()?)
            .with_additional_tasks(1)
            .build_with_tasks(blockchain_db.clone(), ctx.task_executor.clone(), blob_store.clone());

        let transaction_pool =
            reth_transaction_pool::Pool::eth_pool(validator, blob_store, self.txpool.pool_config());
        info!(target: "reth::cli", "Transaction pool initialized");

        // spawn txpool maintenance task
        {
            let pool = transaction_pool.clone();
            let chain_events = blockchain_db.canonical_state_stream();
            let client = blockchain_db.clone();
            ctx.task_executor.spawn_critical(
                "txpool maintenance task",
                reth_transaction_pool::maintain::maintain_transaction_pool_future(
                    client,
                    pool,
                    chain_events,
                    ctx.task_executor.clone(),
                    Default::default(),
                ),
            );
            debug!(target: "reth::cli", "Spawned txpool maintenance task");
        }

        info!(target: "reth::cli", "Connecting to P2P network");
        let network_secret_path = self
            .network
            .p2p_secret_key
            .clone()
            .unwrap_or_else(|| self.data_dir().p2p_secret_path());
        debug!(target: "reth::cli", ?network_secret_path, "Loading p2p key file");
        let secret_key = get_secret_key(&network_secret_path)?;
        let default_peers_path = self.data_dir().known_peers_path();

        // Set up block import structures
        let (block_import_tx, block_import_rx) = unbounded_channel();
        let block_import = ProofOfAuthorityBlockImport::new(self.chain.clone(), block_import_tx);

        let network_config = self.load_network_config(
            &config,
            ctx.task_executor.clone(),
            head,
            secret_key,
            default_peers_path.clone(),
            &self.chain,
            Box::new(block_import.clone()),
            provider_factory.clone(),
        );

        let network_client = network_config.client.clone();
        let mut network_builder = NetworkManager::builder(network_config).await?;

        let components = RethNodeComponentsImpl {
            provider: blockchain_db.clone(),
            pool: transaction_pool.clone(),
            network: network_builder.handle(),
            task_executor: ctx.task_executor.clone(),
            events: blockchain_db.clone(),
        };

        // allow network modifications
        self.ext.configure_network(network_builder.network_mut(), &components)?;

        // launch network
        let network = self.start_network(
            network_builder,
            &ctx.task_executor,
            transaction_pool.clone(),
            network_client,
            default_peers_path,
        );

        debug!(target: "reth::cli", "Spawning payload builder service");
        let payload_builder = self.ext.spawn_payload_builder_service(&self.builder, &components)?;

        // Configure the pipeline
        let (consensus_engine_tx, consensus_engine_rx) = unbounded_channel();
        let (_, mut block_production_task, mut block_fetcher_task, mut sync_controller) =
            AuthorityConsensusBuilder::try_new(
                Arc::clone(&self.chain),
                blockchain_db.clone(),
                transaction_pool.clone(),
                consensus_engine_tx.clone(),
                canon_state_notification_sender.clone(),
                btc_server_client.clone(),
                bitcoin_block_headers_clone,
                self.rpc.btc_block_source.clone(),
                secp256k1::Secp256k1::new(),
                secret_key,
                None,
                network.clone(),
                block_import_rx,
                ctx.task_executor.clone(),
                payload_builder.clone(),
            )
            .expect("Failed to create authority consensus builder")
            .build();

        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), enode = %network.local_node_record(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");
        let network_client = network.fetch_client().await?;

        self.ext.on_components_initialized(&components)?;

        let max_block = if let Some(block) = self.debug.max_block {
            Some(block)
        } else if let Some(tip) = self.debug.tip {
            Some(self.lookup_or_fetch_tip(&db, &network_client, tip).await?)
        } else {
            None
        };

        // Configure the pipeline
        let mut pipeline = self
            .build_networked_pipeline(
                &config.stages,
                network_client.clone(),
                Arc::clone(&authority_consensus),
                provider_factory.clone(),
                &ctx.task_executor,
                sync_metrics_tx,
                prune_config.clone(),
                max_block,
            )
            .await?;

        let pipeline_events = pipeline.events();
        block_production_task.set_pipeline_events(pipeline_events);
        debug!(target: "reth::cli", "Spawning block production task task");

        ctx.task_executor.spawn_critical(
            "PoA Block Production Task",
            Box::pin(async move {
                block_production_task.start_task().await;
            }),
        );

        ctx.task_executor.spawn_critical(
            "PoA Block Fetcher Task",
            Box::pin(async move {
                block_fetcher_task.start_task().await;
            }),
        );

        ctx.task_executor.spawn_critical(
            "PoA Block Sync Controller Task",
            Box::pin(async move {
                sync_controller.start_task().await;
            }),
        );

        let pipeline_events = pipeline.events();

        let initial_target = if let Some(tip) = self.debug.tip {
            // Set the provided tip as the initial pipeline target.
            debug!(target: "reth::cli", %tip, "Tip manually set");
            Some(tip)
        } else if self.debug.continuous {
            // Set genesis as the initial pipeline target.
            // This will allow the downloader to start
            debug!(target: "reth::cli", "Continuous sync mode enabled");
            Some(genesis_hash)
        } else {
            None
        };

        let mut hooks = EngineHooks::new();

        let pruner_events = if let Some(prune_config) = prune_config {
            let mut pruner = self.build_pruner(
                &prune_config,
                db.clone(),
                tree_config,
                snapshotter.highest_snapshot_receiver(),
            );

            let events = pruner.events();
            hooks.add(PruneHook::new(pruner, Box::new(ctx.task_executor.clone())));

            info!(target: "reth::cli", ?prune_config, "Pruner initialized");
            Either::Left(events)
        } else {
            Either::Right(stream::empty())
        };

        // Configure the consensus engine
        let (beacon_consensus_engine, beacon_engine_handle) = BeaconConsensusEngine::with_channel(
            network_client,
            pipeline,
            blockchain_db.clone(),
            Box::new(ctx.task_executor.clone()),
            Box::new(network.clone()),
            max_block,
            self.debug.continuous,
            payload_builder.clone(),
            initial_target,
            MIN_BLOCKS_FOR_PIPELINE_RUN,
            consensus_engine_tx,
            consensus_engine_rx,
            hooks,
        )?;
        info!(target: "reth::cli", "Consensus engine initialized");

        let events = stream_select!(
            network.event_listener().map(Into::into),
            beacon_engine_handle.event_listener().map(Into::into),
            pipeline_events.map(Into::into),
            if self.debug.tip.is_none() {
                Either::Left(
                    ConsensusLayerHealthEvents::new(Box::new(blockchain_db.clone()))
                        .map(Into::into),
                )
            } else {
                Either::Right(stream::empty())
            },
            pruner_events.map(Into::into)
        );
        ctx.task_executor.spawn_critical(
            "events task",
            events::handle_events(Some(network.clone()), Some(head.number), events, db.clone()),
        );

        let engine_api = EngineApi::new(
            blockchain_db.clone(),
            self.chain.clone(),
            beacon_engine_handle,
            payload_builder.into(),
            Box::new(ctx.task_executor.clone()),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // extract the jwt secret from the args if possible
        let default_jwt_path = self.data_dir().jwt_path();
        let jwt_secret = self.rpc.auth_jwt_secret(default_jwt_path)?;

        // adjust rpc port numbers based on instance number
        self.adjust_instance_ports();

        // Start RPC servers
        let _rpc_server_handles =
            self.rpc.start_servers(&components, engine_api, jwt_secret, &mut self.ext).await?;

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        ctx.task_executor.spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

        self.ext.on_node_started(&components)?;
        rx.await??;

        info!(target: "reth::cli", "Consensus engine has exited.");

        if self.debug.terminate {
            Ok(())
        } else {
            // The pipeline has finished downloading blocks up to `--debug.tip` or
            // `--debug.max-block`. Keep other node components alive for further usage.
            futures::future::pending().await
        }
    }

    /// Constructs a [Pipeline] that's wired to the network
    #[allow(clippy::too_many_arguments)]
    async fn build_networked_pipeline<DB, Client>(
        &self,
        config: &StageConfig,
        client: Client,
        consensus: Arc<dyn Consensus>,
        provider_factory: ProviderFactory<DB>,
        task_executor: &TaskExecutor,
        metrics_tx: reth_stages::MetricEventsSender,
        prune_config: Option<PruneConfig>,
        max_block: Option<BlockNumber>,
    ) -> eyre::Result<Pipeline<DB>>
    where
        DB: Database + Unpin + Clone + 'static,
        Client: HeadersClient + BodiesClient + Clone + 'static,
    {
        // building network downloaders using the fetch client
        let header_downloader = ReverseHeadersDownloaderBuilder::from(config.headers)
            .build(client.clone(), Arc::clone(&consensus))
            .into_task_with(task_executor);

        let body_downloader = BodiesDownloaderBuilder::from(config.bodies)
            .build(client, Arc::clone(&consensus), provider_factory.clone())
            .into_task_with(task_executor);

        let pipeline = self
            .build_pipeline(
                provider_factory,
                config,
                header_downloader,
                body_downloader,
                consensus,
                max_block,
                self.debug.continuous,
                metrics_tx,
                prune_config,
            )
            .await?;

        Ok(pipeline)
    }

    /// Returns the chain specific path to the data dir.
    fn data_dir(&self) -> ChainPath<DataDirPath> {
        self.datadir.unwrap_or_chain_default(self.chain.chain)
    }

    /// Returns the path to the config file.
    fn config_path(&self) -> PathBuf {
        self.config.clone().unwrap_or_else(|| self.data_dir().config_path())
    }

    /// Loads the reth config with the given datadir root
    fn load_config(&self) -> eyre::Result<Config> {
        let config_path = self.config_path();
        let mut config = confy::load_path::<Config>(&config_path)
            .wrap_err_with(|| format!("Could not load config file {:?}", config_path))?;

        info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

        // Update the config with the command line arguments
        config.peers.connect_trusted_nodes_only = self.network.trusted_only;

        if !self.network.trusted_peers.is_empty() {
            info!(target: "reth::cli", "Adding trusted nodes");
            self.network.trusted_peers.iter().for_each(|peer| {
                config.peers.trusted_nodes.insert(*peer);
            });
        }

        Ok(config)
    }

    /// Loads the trusted setup params from a given file path or falls back to
    /// `MAINNET_KZG_TRUSTED_SETUP`.
    fn kzg_settings(&self) -> eyre::Result<Arc<KzgSettings>> {
        if let Some(ref trusted_setup_file) = self.trusted_setup_file {
            let trusted_setup = KzgSettings::load_trusted_setup_file(trusted_setup_file)
                .map_err(LoadKzgSettingsError::KzgError)?;
            Ok(Arc::new(trusted_setup))
        } else {
            Ok(Arc::clone(&MAINNET_KZG_TRUSTED_SETUP))
        }
    }

    fn install_prometheus_recorder(&self) -> eyre::Result<PrometheusHandle> {
        prometheus_exporter::install_recorder()
    }

    async fn start_metrics_endpoint(
        &self,
        prometheus_handle: Option<PrometheusHandle>,
        db: Arc<DatabaseEnv>,
    ) -> eyre::Result<()> {
        if let Some(listen_addr) = self.metrics {
            info!(target: "reth::cli", addr = %listen_addr, "Starting metrics endpoint");
            prometheus_exporter::serve(
                listen_addr,
                prometheus_handle.expect("Prometheus handle should be provided"),
                db,
                metrics_process::Collector::default(),
            )
            .await?;
        }

        Ok(())
    }

    /// Spawns the configured network and associated tasks and returns the [NetworkHandle] connected
    /// to that network.
    fn start_network<C, Pool>(
        &self,
        builder: NetworkBuilder<C, (), ()>,
        task_executor: &TaskExecutor,
        pool: Pool,
        client: C,
        default_peers_path: PathBuf,
    ) -> NetworkHandle
    where
        C: BlockReader + HeaderProvider + Clone + Unpin + 'static,
        Pool: TransactionPool + Unpin + 'static,
    {
        let (handle, network, txpool, eth) =
            builder.transactions(pool).request_handler(client).split_with_handle();

        task_executor.spawn_critical("p2p txpool", txpool);
        task_executor.spawn_critical("p2p eth request handler", eth);

        let known_peers_file = self.network.persistent_peers_file(default_peers_path);
        task_executor
            .spawn_critical_with_graceful_shutdown_signal("p2p network task", |shutdown| {
                run_network_until_shutdown(shutdown, network, known_peers_file)
            });

        handle
    }

    /// Fetches the head block from the database.
    ///
    /// If the database is empty, returns the genesis block.
    fn lookup_head<DB: Database>(&self, db: DB) -> RethResult<Head> {
        let factory = ProviderFactory::new(db, self.chain.clone());
        let provider = factory.provider()?;

        let head = provider.get_stage_checkpoint(StageId::Finish)?.unwrap_or_default().block_number;

        let header = provider
            .header_by_number(head)?
            .expect("the header for the latest block is missing, database is corrupt");

        let total_difficulty = provider
            .header_td_by_number(head)?
            .expect("the total difficulty for the latest block is missing, database is corrupt");

        let hash = provider
            .block_hash(head)?
            .expect("the hash for the latest block is missing, database is corrupt");

        Ok(Head {
            number: head,
            hash,
            difficulty: header.difficulty,
            total_difficulty,
            timestamp: header.timestamp,
        })
    }

    /// Attempt to look up the block number for the tip hash in the database.
    /// If it doesn't exist, download the header and return the block number.
    ///
    /// NOTE: The download is attempted with infinite retries.
    async fn lookup_or_fetch_tip<DB, Client>(
        &self,
        db: DB,
        client: Client,
        tip: B256,
    ) -> RethResult<u64>
    where
        DB: Database,
        Client: HeadersClient,
    {
        Ok(self.fetch_tip(db, client, BlockHashOrNumber::Hash(tip)).await?.number)
    }

    /// Attempt to look up the block with the given number and return the header.
    ///
    /// NOTE: The download is attempted with infinite retries.
    async fn fetch_tip<DB, Client>(
        &self,
        db: DB,
        client: Client,
        tip: BlockHashOrNumber,
    ) -> RethResult<SealedHeader>
    where
        DB: Database,
        Client: HeadersClient,
    {
        let factory = ProviderFactory::new(db, self.chain.clone());
        let provider = factory.provider()?;

        let header = provider.header_by_hash_or_number(tip)?;

        // try to look up the header in the database
        if let Some(header) = header {
            info!(target: "reth::cli", ?tip, "Successfully looked up tip block in the database");
            return Ok(header.seal_slow())
        }

        info!(target: "reth::cli", ?tip, "Fetching tip block from the network.");
        loop {
            match get_single_header(&client, tip).await {
                Ok(tip_header) => {
                    info!(target: "reth::cli", ?tip, "Successfully fetched tip");
                    return Ok(tip_header)
                }
                Err(error) => {
                    error!(target: "reth::cli", %error, "Failed to fetch the tip. Retrying...");
                }
            }
        }
    }

    fn load_network_config(
        &self,
        config: &Config,
        executor: TaskExecutor,
        head: Head,
        secret_key: SecretKey,
        default_peers_path: PathBuf,
        chain_spec: &Arc<ChainSpec>,
        block_import: Box<dyn BlockImport>,
        provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
    ) -> NetworkConfig<ProviderFactory<Arc<DatabaseEnv>>> {
        self.network
            .network_config(config, chain_spec.clone(), secret_key, default_peers_path)
            .with_task_executor(Box::new(executor))
            .set_head(head)
            .listener_addr(SocketAddr::V4(SocketAddrV4::new(
                self.network.addr,
                // set discovery port based on instance number
                self.network.port + self.instance - 1,
            )))
            .discovery_addr(SocketAddr::V4(SocketAddrV4::new(
                self.network.addr,
                // set discovery port based on instance number
                self.network.port + self.instance - 1,
            )))
            .network_mode(NetworkMode::Authority)
            .build_with_block_import(provider_factory, block_import)
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_pipeline<DB, H, B>(
        &self,
        provider_factory: ProviderFactory<DB>,
        config: &StageConfig,
        header_downloader: H,
        body_downloader: B,
        consensus: Arc<dyn Consensus>,
        max_block: Option<u64>,
        continuous: bool,
        metrics_tx: reth_stages::MetricEventsSender,
        prune_config: Option<PruneConfig>,
    ) -> eyre::Result<Pipeline<DB>>
    where
        DB: Database + Clone + 'static,
        H: HeaderDownloader + 'static,
        B: BodyDownloader + 'static,
    {
        let mut builder = Pipeline::builder();

        if let Some(max_block) = max_block {
            debug!(target: "reth::cli", max_block, "Configuring builder to use max block");
            builder = builder.with_max_block(max_block)
        }

        let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
        use reth_revm_inspectors::stack::InspectorStackConfig;
        let factory = reth_revm::EvmProcessorFactory::new(self.chain.clone());

        let stack_config = InspectorStackConfig {
            use_printer_tracer: self.debug.print_inspector,
            hook: if let Some(hook_block) = self.debug.hook_block {
                Hook::Block(hook_block)
            } else if let Some(tx) = self.debug.hook_transaction {
                Hook::Transaction(tx)
            } else if self.debug.hook_all {
                Hook::All
            } else {
                Hook::None
            },
        };

        let factory = factory.with_stack_config(stack_config);

        let prune_modes = prune_config.map(|prune| prune.segments).unwrap_or_default();

        let header_mode =
            if continuous { HeaderSyncMode::Continuous } else { HeaderSyncMode::Tip(tip_rx) };
        let pipeline = builder
            .with_tip_sender(tip_tx)
            .with_metrics_tx(metrics_tx.clone())
            .add_stages(
                DefaultStages::new(
                    provider_factory.clone(),
                    header_mode,
                    Arc::clone(&consensus),
                    header_downloader,
                    body_downloader,
                    factory.clone(),
                )
                .set(
                    TotalDifficultyStage::new(consensus)
                        .with_commit_threshold(config.total_difficulty.commit_threshold),
                )
                .set(SenderRecoveryStage {
                    commit_threshold: config.sender_recovery.commit_threshold,
                })
                .set(
                    ExecutionStage::new(
                        factory,
                        ExecutionStageThresholds {
                            max_blocks: config.execution.max_blocks,
                            max_changes: config.execution.max_changes,
                            max_cumulative_gas: config.execution.max_cumulative_gas,
                        },
                        config
                            .merkle
                            .clean_threshold
                            .max(config.account_hashing.clean_threshold)
                            .max(config.storage_hashing.clean_threshold),
                        prune_modes.clone(),
                    )
                    .with_metrics_tx(metrics_tx),
                )
                .set(AccountHashingStage::new(
                    config.account_hashing.clean_threshold,
                    config.account_hashing.commit_threshold,
                ))
                .set(StorageHashingStage::new(
                    config.storage_hashing.clean_threshold,
                    config.storage_hashing.commit_threshold,
                ))
                .set(MerkleStage::new_execution(config.merkle.clean_threshold))
                .set(TransactionLookupStage::new(
                    config.transaction_lookup.commit_threshold,
                    prune_modes.transaction_lookup,
                ))
                .set(IndexAccountHistoryStage::new(
                    config.index_account_history.commit_threshold,
                    prune_modes.account_history,
                ))
                .set(IndexStorageHistoryStage::new(
                    config.index_storage_history.commit_threshold,
                    prune_modes.storage_history,
                )),
            )
            .build(provider_factory);

        Ok(pipeline)
    }

    /// Builds a [Pruner] with the given config.
    fn build_pruner<DB: Database>(
        &self,
        config: &PruneConfig,
        db: DB,
        tree_config: BlockchainTreeConfig,
        highest_snapshots_rx: HighestSnapshotsTracker,
    ) -> Pruner<DB> {
        let segments = SegmentSet::default()
            // Receipts
            .segment_opt(config.segments.receipts.map(reth_prune::segments::Receipts::new))
            // Receipts by logs
            .segment_opt((!config.segments.receipts_log_filter.is_empty()).then(|| {
                reth_prune::segments::ReceiptsByLogs::new(
                    config.segments.receipts_log_filter.clone(),
                )
            }))
            // Transaction lookup
            .segment_opt(
                config
                    .segments
                    .transaction_lookup
                    .map(reth_prune::segments::TransactionLookup::new),
            )
            // Sender recovery
            .segment_opt(
                config.segments.sender_recovery.map(reth_prune::segments::SenderRecovery::new),
            )
            // Account history
            .segment_opt(
                config.segments.account_history.map(reth_prune::segments::AccountHistory::new),
            )
            // Storage history
            .segment_opt(
                config.segments.storage_history.map(reth_prune::segments::StorageHistory::new),
            );

        Pruner::new(
            db,
            self.chain.clone(),
            segments.into_vec(),
            config.block_interval,
            self.chain.prune_delete_limit,
            tree_config.max_reorg_depth() as usize,
            highest_snapshots_rx,
        )
    }

    /// Change rpc port numbers based on the instance number.
    fn adjust_instance_ports(&mut self) {
        // auth port is scaled by a factor of instance * 100
        self.rpc.auth_port += self.instance * 100 - 100;
        // http port is scaled by a factor of -instance
        self.rpc.http_port -= self.instance - 1;
        // ws port is scaled by a factor of instance * 2
        self.rpc.ws_port += self.instance * 2 - 2;
    }
}

/// Drives the [NetworkManager] future until a [Shutdown](reth_tasks::shutdown::Shutdown) signal is
/// received. If configured, this writes known peers to `persistent_peers_file` afterwards.
async fn run_network_until_shutdown<C>(
    shutdown: reth_tasks::shutdown::GracefulShutdown,
    network: NetworkManager<C>,
    persistent_peers_file: Option<PathBuf>,
) where
    C: BlockReader + HeaderProvider + Clone + Unpin + 'static,
{
    pin_mut!(network, shutdown);

    let mut graceful_guard = None;
    tokio::select! {
        _ = &mut network => {},
        guard = shutdown => {
            graceful_guard = Some(guard);
        },
    }

    if let Some(file_path) = persistent_peers_file {
        let known_peers = network.all_peers().collect::<Vec<_>>();
        if let Ok(known_peers) = serde_json::to_string_pretty(&known_peers) {
            trace!(target: "reth::cli", peers_file =?file_path, num_peers=%known_peers.len(), "Saving current peers");
            let parent_dir = file_path.parent().map(std::fs::create_dir_all).transpose();
            match parent_dir.and_then(|_| std::fs::write(&file_path, known_peers)) {
                Ok(_) => {
                    info!(target: "reth::cli", peers_file=?file_path, "Wrote network peers to file");
                }
                Err(err) => {
                    warn!(target: "reth::cli", ?err, peers_file=?file_path, "Failed to write network peers to file");
                }
            }
        }
    }

    drop(graceful_guard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::utils::SUPPORTED_CHAINS;
    use reth_discv4::DEFAULT_DISCOVERY_PORT;
    use std::{
        net::{IpAddr, Ipv4Addr},
        path::Path,
    };

    #[test]
    fn parse_help_node_command() {
        let err = NodeCommand::<()>::try_parse_from(["reth", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    #[test]
    fn parse_common_node_command_chain_args() {
        for chain in SUPPORTED_CHAINS {
            let args: NodeCommand = NodeCommand::<()>::parse_from(["reth", "--chain", chain]);
            assert_eq!(args.chain.chain, chain.parse().unwrap());
        }
    }

    #[test]
    fn parse_discovery_addr() {
        let cmd =
            NodeCommand::<()>::try_parse_from(["reth", "--discovery.addr", "127.0.0.1"]).unwrap();
        assert_eq!(cmd.network.discovery.addr, Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn parse_addr() {
        let cmd = NodeCommand::<()>::try_parse_from([
            "reth",
            "--discovery.addr",
            "127.0.0.1",
            "--addr",
            "127.0.0.1",
        ])
        .unwrap();
        assert_eq!(cmd.network.discovery.addr, Ipv4Addr::LOCALHOST);
        assert_eq!(cmd.network.addr, Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn parse_discovery_port() {
        let cmd = NodeCommand::<()>::try_parse_from(["reth", "--discovery.port", "300"]).unwrap();
        assert_eq!(cmd.network.discovery.port, 300);
    }

    #[test]
    fn parse_port() {
        let cmd =
            NodeCommand::<()>::try_parse_from(["reth", "--discovery.port", "300", "--port", "99"])
                .unwrap();
        assert_eq!(cmd.network.discovery.port, 300);
        assert_eq!(cmd.network.port, 99);
    }

    #[test]
    fn parse_metrics_port() {
        let cmd = NodeCommand::<()>::try_parse_from(["reth", "--metrics", "9001"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));

        let cmd = NodeCommand::<()>::try_parse_from(["reth", "--metrics", ":9001"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));

        let cmd =
            NodeCommand::<()>::try_parse_from(["reth", "--metrics", "localhost:9001"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));
    }

    #[test]
    fn parse_config_path() {
        let cmd = NodeCommand::<()>::try_parse_from(["reth", "--config", "my/path/to/reth.toml"])
            .unwrap();
        // always store reth.toml in the data dir, not the chain specific data dir
        let data_dir = cmd.datadir.unwrap_or_chain_default(cmd.chain.chain);
        let config_path = cmd.config.unwrap_or(data_dir.config_path());
        assert_eq!(config_path, Path::new("my/path/to/reth.toml"));

        let cmd = NodeCommand::<()>::try_parse_from(["reth"]).unwrap();

        // always store reth.toml in the data dir, not the chain specific data dir
        let data_dir = cmd.datadir.unwrap_or_chain_default(cmd.chain.chain);
        let config_path = cmd.config.clone().unwrap_or(data_dir.config_path());
        let end = format!("reth/{}/reth.toml", SUPPORTED_CHAINS[0]);
        assert!(config_path.ends_with(end), "{:?}", cmd.config);
    }

    #[test]
    fn parse_db_path() {
        let cmd = NodeCommand::<()>::try_parse_from(["reth"]).unwrap();
        let data_dir = cmd.datadir.unwrap_or_chain_default(cmd.chain.chain);
        let db_path = data_dir.db_path();
        let end = format!("reth/{}/db", SUPPORTED_CHAINS[0]);
        assert!(db_path.ends_with(end), "{:?}", cmd.config);

        let cmd =
            NodeCommand::<()>::try_parse_from(["reth", "--datadir", "my/custom/path"]).unwrap();
        let data_dir = cmd.datadir.unwrap_or_chain_default(cmd.chain.chain);
        let db_path = data_dir.db_path();
        assert_eq!(db_path, Path::new("my/custom/path/db"));
    }

    #[test]
    #[cfg(not(feature = "optimism"))] // dev mode not yet supported in op-reth
    fn parse_dev() {
        let cmd = NodeCommand::<()>::parse_from(["reth", "--dev"]);
        let chain = reth_primitives::DEV.clone();
        assert_eq!(cmd.chain.chain, chain.chain);
        assert_eq!(cmd.chain.genesis_hash, chain.genesis_hash);
        assert_eq!(
            cmd.chain.paris_block_and_final_difficulty,
            chain.paris_block_and_final_difficulty
        );
        assert_eq!(cmd.chain.hardforks, chain.hardforks);

        assert!(cmd.rpc.http);
        assert!(cmd.network.discovery.disable_discovery);

        assert!(cmd.dev.dev);
    }

    #[test]
    fn parse_instance() {
        let mut cmd = NodeCommand::<()>::parse_from(["reth"]);
        cmd.adjust_instance_ports();
        cmd.network.port = DEFAULT_DISCOVERY_PORT + cmd.instance - 1;
        // check rpc port numbers
        assert_eq!(cmd.rpc.auth_port, 8551);
        assert_eq!(cmd.rpc.http_port, 8545);
        assert_eq!(cmd.rpc.ws_port, 8546);
        // check network listening port number
        assert_eq!(cmd.network.port, 30303);

        let mut cmd = NodeCommand::<()>::parse_from(["reth", "--instance", "2"]);
        cmd.adjust_instance_ports();
        cmd.network.port = DEFAULT_DISCOVERY_PORT + cmd.instance - 1;
        // check rpc port numbers
        assert_eq!(cmd.rpc.auth_port, 8651);
        assert_eq!(cmd.rpc.http_port, 8544);
        assert_eq!(cmd.rpc.ws_port, 8548);
        // check network listening port number
        assert_eq!(cmd.network.port, 30304);

        let mut cmd = NodeCommand::<()>::parse_from(["reth", "--instance", "3"]);
        cmd.adjust_instance_ports();
        cmd.network.port = DEFAULT_DISCOVERY_PORT + cmd.instance - 1;
        // check rpc port numbers
        assert_eq!(cmd.rpc.auth_port, 8751);
        assert_eq!(cmd.rpc.http_port, 8543);
        assert_eq!(cmd.rpc.ws_port, 8550);
        // check network listening port number
        assert_eq!(cmd.network.port, 30305);
    }
}
