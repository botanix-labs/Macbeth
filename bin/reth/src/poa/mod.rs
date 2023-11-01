//! Poa node executable
//!
//! Starts the poa client
use crate::{
    args::{
        get_secret_key, utils::parse_socket_address, DatabaseArgs, DebugArgs, NetworkArgs,
        PayloadBuilderArgs, PruningArgs, RpcServerArgs, TxPoolArgs,
    },
    cli::{
        config::RethRpcConfig,
        ext::{RethCliExt, RethNodeCommandConfig},
    },
    dirs::{DataDirPath, MaybePlatformPath},
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
use hex::FromHex;
use reth_authority_consensus::{AuthorityConsensus, AuthorityConsensusBuilder};

use reth_beacon_consensus::{BeaconConsensusEngine, MIN_BLOCKS_FOR_PIPELINE_RUN};
use reth_blockchain_tree::{
    config::BlockchainTreeConfig, externals::TreeExternals, BlockchainTree, ShareableBlockchainTree,
};
use reth_config::{config::PruneConfig, Config};
use reth_db::{database::Database, init_db, DatabaseEnv};
use reth_discv4::DEFAULT_DISCOVERY_PORT;
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
};
use reth_network::{error::NetworkError, NetworkConfig, NetworkHandle, NetworkManager};
use reth_network_api::NetworkInfo;
use reth_primitives::{
    constants::eip4844::{LoadKzgSettingsError, MAINNET_KZG_TRUSTED_SETUP},
    kzg::KzgSettings,
    stage::StageId,
    BlockHashOrNumber, BlockNumber, ChainSpec, Head, SealedHeader, BOTANIX_TESTNET, H256,
};
use reth_provider::{
    providers::BlockchainProvider, BlockHashReader, BlockReader, CanonStateSubscriptions,
    HeaderProvider, ProviderFactory, StageCheckpointReader,
};
use reth_revm::Factory;
use reth_revm_inspectors::stack::Hook;
use reth_rpc_engine_api::EngineApi;
use reth_stages::{
    prelude::*,
    stages::{
        AccountHashingStage, ExecutionStage, ExecutionStageThresholds, HeaderSyncMode,
        IndexAccountHistoryStage, IndexStorageHistoryStage, MerkleStage, SenderRecoveryStage,
        StorageHashingStage, TotalDifficultyStage, TransactionLookupStage,
    },
    MetricEventsSender, MetricsListener,
};
use reth_tasks::TaskExecutor;
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, TransactionPool, TransactionValidationTaskExecutor,
};
use secp256k1::SecretKey;
use std::{
    fs::File,
    io::Read,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{mpsc::unbounded_channel, oneshot, watch, RwLock};
use tracing::*;

use btc_wallet::block_source::{BlockSource, MempoolSpace};
use client::BtcServerClient;

use crate::node::events;

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

    /// Additional cli arguments
    #[clap(flatten)]
    pub ext: Ext::Node,

    /// All pruning related arguments
    #[clap(flatten)]
    pub pruning: PruningArgs,
}

impl<Ext: RethCliExt> PoaNodeCommand<Ext> {
    /// Replaces the extension of the node command
    pub fn with_ext<E: RethCliExt>(self, ext: E::Node) -> PoaNodeCommand<E> {
        let Self {
            datadir,
            config,
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
            metrics,
            instance,
            trusted_setup_file,
            network,
            rpc,
            txpool,
            builder,
            debug,
            db,
            ext,
            pruning,
        }
    }

    /// Execute `node` command
    pub async fn execute(mut self, ctx: CliContext) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        // Raise the fd limit of the process.
        // Does not do anything on windows.
        raise_fd_limit();

        // add network name to data dir
        let botanix_chain_spec = BOTANIX_TESTNET.clone();
        let data_dir = self.datadir.unwrap_or_chain_default(botanix_chain_spec.clone().chain);
        let config_path = self.config.clone().unwrap_or(data_dir.config_path());

        let mut config: Config = self.load_config(config_path.clone())?;
        // Read the trusted setup file
        // TODO this should be moved to HSM abstraction
        let network_secret_path =
            self.network.p2p_secret_key.clone().unwrap_or_else(|| data_dir.p2p_secret_path());
        debug!(target: "reth::cli", ?network_secret_path, "Loading p2p key file");
        let secret_key = get_secret_key(&network_secret_path)?;

        let prune_config =
            self.pruning.prune_config(Arc::clone(&botanix_chain_spec))?.or(config.prune.clone());

        // Connect to btc signining server
        let btc_server_client: BtcServerClient<tonic::transport::Channel> =
            BtcServerClient::connect(self.rpc.btc_server.clone())
                .await
                .expect("connect to btc_server");
        info!(target: "reth::cli", "Btc server connected");

        let bitcoin_block_headers: Arc<RwLock<Option<bitcoin::block::Header>>> =
            Arc::new(RwLock::new(None));
        let bitcoin_block_headers_clone = bitcoin_block_headers.clone();
        let block_source = MempoolSpace::new(self.rpc.btc_block_source.to_string().clone());

        // Spawns a critical task that asynchronously retrieves Bitcoin block headers.
        // The retrieved block headers are stored in a shared `RwLock` for later use.
        // The task runs indefinitely with a sleep time of 5 seconds between each iteration.
        ctx.task_executor.spawn_critical(
            "async bitcoin block header task",
            Box::pin(async move {
                let sleep_ms = tokio::time::Duration::from_millis(5000);
                let mut tip = 0u64;
                loop {
                    let mut header_write = bitcoin_block_headers.write().await;
                    let current_tip = block_source.get_tip().await.unwrap();
                    if current_tip != tip {
                        info!("Async bitcoin worker tip mismatch");
                        let block_hash = block_source.get_block_hash(current_tip).await.unwrap();
                        let block_header = block_source.get_block_header(block_hash).await.unwrap();

                        // TODO (armins) in v1 we will need the nth deep block header not tip
                        *header_write = Some(block_header);
                        tip = current_tip;
                    }
                    tokio::time::sleep(sleep_ms).await;
                }
            }),
        );
        info!(target: "reth::cli", "Spawned async bitcoin block header task");

        // always store reth.toml in the data dir, not the chain specific data dir
        info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

        let db_path = data_dir.db_path();
        info!(target: "reth::cli", path = ?db_path, "Opening database");
        let db = Arc::new(init_db(&db_path, self.db.log_level)?);
        info!(target: "reth::cli", "Database opened");

        self.start_metrics_endpoint(Arc::clone(&db)).await?;

        let genesis_hash = init_genesis(db.clone(), botanix_chain_spec.clone())?;
        let consensus = Arc::new(AuthorityConsensus::new(Arc::clone(&botanix_chain_spec)));

        self.init_trusted_nodes(&mut config);

        // Start mentrics listener task
        debug!(target: "reth::cli", "Spawning metrics listener task");
        let (metrics_tx, metrics_rx) = unbounded_channel();
        let metrics_listener = MetricsListener::new(metrics_rx);
        ctx.task_executor.spawn_critical("metrics listener task", metrics_listener);

        // configure blockchain tree
        let tree_externals = TreeExternals::new(
            db.clone(),
            Arc::clone(&consensus),
            Factory::new(botanix_chain_spec.clone()),
            Arc::clone(&botanix_chain_spec),
        );
        let tree_config = BlockchainTreeConfig::default();
        // The size of the broadcast is twice the maximum reorg depth, because at maximum reorg
        // depth at least N blocks must be sent at once.
        let (canon_state_notification_sender, _receiver) =
            tokio::sync::broadcast::channel(tree_config.max_reorg_depth() as usize * 2);
        let blockchain_tree = ShareableBlockchainTree::new(
            BlockchainTree::new(
                tree_externals,
                canon_state_notification_sender.clone(),
                tree_config,
                prune_config.clone().map(|config| config.parts),
            )?
            .with_sync_metrics_tx(metrics_tx.clone()),
        );

        // setup the blockchain provider
        let factory = ProviderFactory::new(Arc::clone(&db), Arc::clone(&botanix_chain_spec));
        let blockchain_db = BlockchainProvider::new(factory, blockchain_tree.clone())?;
        let blob_store = InMemoryBlobStore::default();
        let validator =
            TransactionValidationTaskExecutor::eth_builder(Arc::clone(&botanix_chain_spec))
                .kzg_settings(self.kzg_settings()?)
                .with_additional_tasks(1)
                .build_with_tasks(
                    blockchain_db.clone(),
                    ctx.task_executor.clone(),
                    blob_store.clone(),
                );

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

        let default_peers_path = data_dir.known_peers_path();
        let head = self
            .lookup_head(Arc::clone(&db), &botanix_chain_spec)
            .expect("the head block is missing");
        let network_config = self.load_network_config(
            &config,
            Arc::clone(&db),
            ctx.task_executor.clone(),
            head,
            secret_key,
            default_peers_path.clone(),
            &botanix_chain_spec,
        );
        let network = self
            .start_network(
                network_config,
                &ctx.task_executor,
                transaction_pool.clone(),
                default_peers_path,
            )
            .await?;
        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");
        let network_client = network.fetch_client().await?;

        let (consensus_engine_tx, consensus_engine_rx) = unbounded_channel();

        debug!(target: "reth::cli", "Spawning payload builder service");
        let payload_builder = self.ext.spawn_payload_builder_service(
            &self.builder,
            blockchain_db.clone(),
            transaction_pool.clone(),
            ctx.task_executor.clone(),
            Arc::clone(&botanix_chain_spec),
        )?;

        let max_block = if let Some(block) = self.debug.max_block {
            Some(block)
        } else if let Some(tip) = self.debug.tip {
            Some(self.lookup_or_fetch_tip(&db, &network_client, tip, &botanix_chain_spec).await?)
        } else {
            None
        };

        let authority_consensus: Arc<dyn Consensus> =
            Arc::new(AuthorityConsensus::new(botanix_chain_spec.clone()));
        // Configure the pipeline
        let (_, authority_client, mut block_production_task) = AuthorityConsensusBuilder::new(
            Arc::clone(&botanix_chain_spec),
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
        )
        .build();

        let mut pipeline = self
            .build_networked_pipeline(
                &config,
                authority_client.clone(),
                Arc::clone(&authority_consensus),
                db.clone(),
                &ctx.task_executor,
                metrics_tx,
                prune_config.clone(),
                max_block,
                &botanix_chain_spec.clone(),
            )
            .await?;

        let pipeline_events = pipeline.events();
        block_production_task.set_pipeline_events(pipeline_events);
        debug!(target: "reth::cli", "Spawning block production task task");
        ctx.task_executor.spawn(Box::pin(block_production_task));

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

        let pruner = prune_config.map(|prune_config| {
            info!(target: "reth::cli", "Pruner initialized");
            reth_prune::Pruner::new(
                db.clone(),
                botanix_chain_spec.clone(),
                prune_config.block_interval,
                prune_config.parts,
                botanix_chain_spec.prune_batch_sizes,
            )
        });

        // Configure the consensus engine
        let (beacon_consensus_engine, beacon_engine_handle) = BeaconConsensusEngine::with_channel(
            authority_client,
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
            pruner,
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
            }
        );
        ctx.task_executor.spawn_critical(
            "events task",
            events::handle_events(Some(network.clone()), Some(head.number), events),
        );

        let engine_api = EngineApi::new(
            blockchain_db.clone(),
            botanix_chain_spec.clone(),
            beacon_engine_handle,
            payload_builder.into(),
            Box::new(ctx.task_executor.clone()),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // extract the jwt secret from the args if possible
        let default_jwt_path = data_dir.jwt_path();
        let jwt_secret = self.rpc.jwt_secret(default_jwt_path)?;

        // adjust rpc port numbers based on instance number
        self.adjust_instance_ports();

        // Start RPC servers
        let (_rpc_server, _auth_server) = self
            .rpc
            .start_servers(
                blockchain_db.clone(),
                transaction_pool.clone(),
                network.clone(),
                ctx.task_executor.clone(),
                blockchain_tree,
                engine_api,
                jwt_secret,
                &mut self.ext,
            )
            .await?;

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        ctx.task_executor.spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

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
        config: &Config,
        client: Client,
        consensus: Arc<dyn Consensus>,
        db: DB,
        task_executor: &TaskExecutor,
        metrics_tx: MetricEventsSender,
        prune_config: Option<PruneConfig>,
        max_block: Option<BlockNumber>,
        chain_spec: &Arc<ChainSpec>,
    ) -> eyre::Result<Pipeline<DB>>
    where
        DB: Database + Unpin + Clone + 'static,
        Client: HeadersClient + BodiesClient + Clone + 'static,
    {
        // building network downloaders using the fetch client
        let header_downloader = ReverseHeadersDownloaderBuilder::from(config.stages.headers)
            .build(client.clone(), Arc::clone(&consensus))
            .into_task_with(task_executor);

        let body_downloader = BodiesDownloaderBuilder::from(config.stages.bodies)
            .build(client, Arc::clone(&consensus), db.clone())
            .into_task_with(task_executor);

        let pipeline = self
            .build_pipeline(
                db,
                config,
                header_downloader,
                body_downloader,
                consensus,
                max_block,
                self.debug.continuous,
                metrics_tx,
                prune_config,
                chain_spec,
            )
            .await?;

        Ok(pipeline)
    }

    /// Loads the reth config with the given datadir root
    fn load_config(&self, config_path: PathBuf) -> eyre::Result<Config> {
        confy::load_path::<Config>(config_path.clone())
            .wrap_err_with(|| format!("Could not load config file {:?}", config_path))
    }

    /// Loads the trusted setup params from a given file path or falls back to
    /// `MAINNET_KZG_TRUSTED_SETUP`.
    fn kzg_settings(&self) -> eyre::Result<Arc<KzgSettings>> {
        if let Some(ref trusted_setup_file) = self.trusted_setup_file {
            let trusted_setup = KzgSettings::load_trusted_setup_file(trusted_setup_file.into())
                .map_err(LoadKzgSettingsError::KzgError)?;
            Ok(Arc::new(trusted_setup))
        } else {
            Ok(Arc::clone(&MAINNET_KZG_TRUSTED_SETUP))
        }
    }

    fn init_trusted_nodes(&self, config: &mut Config) {
        config.peers.connect_trusted_nodes_only = self.network.trusted_only;

        if !self.network.trusted_peers.is_empty() {
            info!(target: "reth::cli", "Adding trusted nodes");
            self.network.trusted_peers.iter().for_each(|peer| {
                config.peers.trusted_nodes.insert(*peer);
            });
        }
    }

    async fn start_metrics_endpoint(&self, db: Arc<DatabaseEnv>) -> eyre::Result<()> {
        if let Some(listen_addr) = self.metrics {
            info!(target: "reth::cli", addr = %listen_addr, "Starting metrics endpoint");
            prometheus_exporter::initialize(listen_addr, db, metrics_process::Collector::default())
                .await?;
        }

        Ok(())
    }

    /// Spawns the configured network and associated tasks and returns the [NetworkHandle] connected
    /// to that network.
    async fn start_network<C, Pool>(
        &self,
        config: NetworkConfig<C>,
        task_executor: &TaskExecutor,
        pool: Pool,
        default_peers_path: PathBuf,
    ) -> Result<NetworkHandle, NetworkError>
    where
        C: BlockReader + HeaderProvider + Clone + Unpin + 'static,
        Pool: TransactionPool + Unpin + 'static,
    {
        let client = config.client.clone();
        let (handle, network, txpool, eth) = NetworkManager::builder(config)
            .await?
            .transactions(pool)
            .request_handler(client)
            .split_with_handle();

        task_executor.spawn_critical("p2p txpool", txpool);
        task_executor.spawn_critical("p2p eth request handler", eth);

        let known_peers_file = self.network.persistent_peers_file(default_peers_path);
        task_executor.spawn_critical_with_signal("p2p network task", |shutdown| {
            run_network_until_shutdown(shutdown, network, known_peers_file)
        });

        Ok(handle)
    }

    fn lookup_head(
        &self,
        db: Arc<DatabaseEnv>,
        chain_spec: &Arc<ChainSpec>,
    ) -> Result<Head, reth_interfaces::Error> {
        let factory = ProviderFactory::new(db, chain_spec.clone());
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
        db: &DB,
        client: Client,
        tip: H256,
        chain_spec: &Arc<ChainSpec>,
    ) -> Result<u64, reth_interfaces::Error>
    where
        DB: Database,
        Client: HeadersClient,
    {
        Ok(self.fetch_tip(db, client, BlockHashOrNumber::Hash(tip), chain_spec).await?.number)
    }

    /// Attempt to look up the block with the given number and return the header.
    ///
    /// NOTE: The download is attempted with infinite retries.
    async fn fetch_tip<DB, Client>(
        &self,
        db: &DB,
        client: Client,
        tip: BlockHashOrNumber,
        chain_spec: &Arc<ChainSpec>,
    ) -> Result<SealedHeader, reth_interfaces::Error>
    where
        DB: Database,
        Client: HeadersClient,
    {
        let factory = ProviderFactory::new(db, chain_spec.clone());
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

    fn load_secret_key(&self, secret_key_path: PathBuf) -> eyre::Result<SecretKey> {
        let mut file = File::open(secret_key_path)?;
        // Read the contents of the file into a Vec<u8>
        let mut hex_data: Vec<_> = Vec::new();
        file.read_to_end(&mut hex_data)?;

        // Parse the hex data into bytes
        let secret_bytes = Vec::from_hex(hex_data)?;
        let sk = secp256k1::SecretKey::from_slice(&secret_bytes)
            .map_err(|_| eyre::eyre!("Invalid secret key file"))?;

        Ok(sk)
    }

    fn load_network_config(
        &self,
        config: &Config,
        db: Arc<DatabaseEnv>,
        executor: TaskExecutor,
        head: Head,
        secret_key: SecretKey,
        default_peers_path: PathBuf,
        chain_spec: &Arc<ChainSpec>,
    ) -> NetworkConfig<ProviderFactory<Arc<DatabaseEnv>>> {
        self.network
            .network_config(config, chain_spec.clone(), secret_key, default_peers_path)
            .with_task_executor(Box::new(executor))
            .set_head(head)
            .listener_addr(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::UNSPECIFIED,
                // set discovery port based on instance number
                match self.network.port {
                    Some(port) => port + self.instance - 1,
                    None => DEFAULT_DISCOVERY_PORT + self.instance - 1,
                },
            )))
            .discovery_addr(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::UNSPECIFIED,
                // set discovery port based on instance number
                match self.network.port {
                    Some(port) => port + self.instance - 1,
                    None => DEFAULT_DISCOVERY_PORT + self.instance - 1,
                },
            )))
            .build(ProviderFactory::new(db, chain_spec.clone()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_pipeline<DB, H, B>(
        &self,
        db: DB,
        config: &Config,
        header_downloader: H,
        body_downloader: B,
        consensus: Arc<dyn Consensus>,
        max_block: Option<u64>,
        continuous: bool,
        metrics_tx: MetricEventsSender,
        prune_config: Option<PruneConfig>,
        chain_spec: &Arc<ChainSpec>,
    ) -> eyre::Result<Pipeline<DB>>
    where
        DB: Database + Clone + 'static,
        H: HeaderDownloader + 'static,
        B: BodyDownloader + 'static,
    {
        let stage_config = &config.stages;

        let mut builder = Pipeline::builder();

        if let Some(max_block) = max_block {
            debug!(target: "reth::cli", max_block, "Configuring builder to use max block");
            builder = builder.with_max_block(max_block)
        }

        let (tip_tx, tip_rx) = watch::channel(H256::zero());
        use reth_revm_inspectors::stack::InspectorStackConfig;
        let factory = reth_revm::Factory::new(chain_spec.clone());

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

        let prune_modes = prune_config.map(|prune| prune.parts).unwrap_or_default();

        let header_mode =
            if continuous { HeaderSyncMode::Continuous } else { HeaderSyncMode::Tip(tip_rx) };
        let pipeline = builder
            .with_tip_sender(tip_tx)
            .with_metrics_tx(metrics_tx.clone())
            .add_stages(
                DefaultStages::new(
                    header_mode,
                    Arc::clone(&consensus),
                    header_downloader,
                    body_downloader,
                    factory.clone(),
                )
                .set(
                    TotalDifficultyStage::new(consensus)
                        .with_commit_threshold(stage_config.total_difficulty.commit_threshold),
                )
                .set(SenderRecoveryStage {
                    commit_threshold: stage_config.sender_recovery.commit_threshold,
                })
                .set(
                    ExecutionStage::new(
                        factory,
                        ExecutionStageThresholds {
                            max_blocks: stage_config.execution.max_blocks,
                            max_changes: stage_config.execution.max_changes,
                        },
                        stage_config
                            .merkle
                            .clean_threshold
                            .max(stage_config.account_hashing.clean_threshold)
                            .max(stage_config.storage_hashing.clean_threshold),
                        prune_modes.clone(),
                    )
                    .with_metrics_tx(metrics_tx),
                )
                .set(AccountHashingStage::new(
                    stage_config.account_hashing.clean_threshold,
                    stage_config.account_hashing.commit_threshold,
                ))
                .set(StorageHashingStage::new(
                    stage_config.storage_hashing.clean_threshold,
                    stage_config.storage_hashing.commit_threshold,
                ))
                .set(MerkleStage::new_execution(stage_config.merkle.clean_threshold))
                .set(TransactionLookupStage::new(
                    stage_config.transaction_lookup.commit_threshold,
                    prune_modes.clone(),
                ))
                .set(IndexAccountHistoryStage::new(
                    stage_config.index_account_history.commit_threshold,
                    prune_modes.clone(),
                ))
                .set(IndexStorageHistoryStage::new(
                    stage_config.index_storage_history.commit_threshold,
                    prune_modes,
                )),
            )
            .build(db, chain_spec.clone());

        Ok(pipeline)
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
    shutdown: reth_tasks::shutdown::Shutdown,
    network: NetworkManager<C>,
    persistent_peers_file: Option<PathBuf>,
) where
    C: BlockReader + HeaderProvider + Clone + Unpin + 'static,
{
    pin_mut!(network, shutdown);

    tokio::select! {
        _ = &mut network => {},
        _ = shutdown => {},
    }

    if let Some(file_path) = persistent_peers_file {
        let known_peers = network.all_peers().collect::<Vec<_>>();
        if let Ok(known_peers) = serde_json::to_string_pretty(&known_peers) {
            trace!(target : "reth::cli", peers_file =?file_path, num_peers=%known_peers.len(), "Saving current peers");
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
}
