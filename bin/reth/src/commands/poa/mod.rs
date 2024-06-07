//! Main node command

use std::{
    borrow::Cow, ffi::OsString, fmt, net::SocketAddr, path::PathBuf, sync::Arc, time::Instant,
};

use bitcoin::hashes::Hash;
use clap::{value_parser, Parser};
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::{stream_select, StreamExt};
use reth_authority_consensus::{
    extended_client::BtcServerExtendedClient, utils::retry_exec, AuthorityConsensus,
    AuthorityConsensusBuilder,
};
use reth_network_types::pk2id;
use secp256k1::{PublicKey, SecretKey, SECP256K1};

use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_beacon_consensus::{
    hooks::EngineHooks, BeaconConsensusEngine, MIN_BLOCKS_FOR_PIPELINE_RUN,
};
use reth_blockchain_tree::{
    BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals,
};
use reth_btc_wallet::bitcoind::{BitcoindClient, BitcoindConfig};
use reth_cli_runner::CliContext;
use reth_config::{config::StageConfig, Config};
use reth_consensus::Consensus;
use reth_consensus_common::{utils, utils::get_authority_signer_index};
use reth_db::{database::Database, init_db, DatabaseEnv};
use reth_exex::ExExManagerHandle;
use reth_interfaces::sync::SyncStateProvider;
use reth_network::{
    frost::manager::FrostConfig, import::ProofOfAuthorityBlockImport, NetworkEvents, NetworkManager,
};
use reth_node_builder::{
    setup::build_networked_pipeline, PayloadBuilderConfig, RethRpcConfig, RethTransactionPoolConfig,
};
use reth_node_core::{args::get_secret_key, init::init_genesis, node_config::NodeConfig, version};
use reth_node_ethereum::EthEvmConfig;
use reth_node_events::node::handle_events;
use reth_primitives::{
    constants::{eip4844::MAINNET_KZG_TRUSTED_SETUP, ETHEREUM_BLOCK_GAS_LIMIT},
    kzg::KzgSettings,
    stage::StageId,
    Bytes, ChainSpec, Head, PruneModes,
};
use reth_provider::{
    providers::{BlockchainProvider, StaticFileProvider},
    BlockHashReader, CanonStateSubscriptions, HeaderProvider, ProviderFactory,
    StageCheckpointReader,
};
use reth_revm::EvmProcessorFactory;
use reth_rpc::EngineApi;
use reth_static_file::StaticFileProducer;
use reth_transaction_pool::{blobstore::InMemoryBlobStore, TransactionValidationTaskExecutor};
use rsntp::AsyncSntpClient;
use tokio::{
    sync::{mpsc::unbounded_channel, oneshot, RwLock},
    time::Duration,
};
use tracing::{debug, error, info};

use crate::{
    args::{
        utils::{
            chain_help, genesis_value_parser, get_federation_pks_from_path, parse_socket_address,
            SUPPORTED_CHAINS,
        },
        DatabaseArgs, DebugArgs, DevArgs, NetworkArgs, PayloadBuilderArgs, PruningArgs,
        RpcServerArgs, TxPoolArgs,
    },
    cli::ext::{NoArgs, PoaNodeCommandConfig, RethNodeComponents},
    dirs::{DataDirPath, MaybePlatformPath},
    payload::PayloadBuilderService,
    rpc::types::NodeRecord,
};
use std::str::FromStr;

/// Enum representing the node sync status
#[derive(Debug, Copy, Clone)]
pub enum LiveSyncStatus {
    /// Node is still syncing
    Syncing,
    /// Noe is fully synced
    Synced,
}

/// Start the node
#[derive(Debug, Parser)]
pub struct PoaNodeCommand<Ext: clap::Args + fmt::Debug = NoArgs> {
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

    /// Run in federation mode. Only the nodes in the federation will be able to produce blocks.
    #[arg(long, value_name = "FEDERATION_MODE", default_value = "false")]
    pub federation_mode: bool,

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

    /// Sets all ports to unused, allowing the OS to choose random unused ports when sockets are
    /// bound.
    ///
    /// Mutually exclusive with `--instance`.
    #[arg(long, conflicts_with = "instance", global = true)]
    pub with_unused_ports: bool,

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

    /// All dev related arguments with --dev prefix
    #[command(flatten)]
    pub dev: DevArgs,

    /// All pruning related arguments
    #[clap(flatten)]
    pub pruning: PruningArgs,

    /// Additional cli arguments
    #[command(flatten, next_help_heading = "Extension")]
    pub ext: Ext,
}

impl PoaNodeCommand {
    /// Parsers only the default CLI arguments
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Parsers only the default [PoaNodeCommand] arguments from the given iterator
    pub fn try_parse_args_from<I, T>(itr: I) -> Result<Self, clap::error::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        Self::try_parse_from(itr)
    }
}

impl<Ext: clap::Args + fmt::Debug + PoaNodeCommandConfig> PoaNodeCommand<Ext> {
    /// Replaces the extension of the node command
    pub fn with_ext<E: clap::Args + fmt::Debug>(self, ext: E) -> PoaNodeCommand<E> {
        let Self {
            datadir,
            config,
            chain,
            federation_mode,
            metrics,
            instance,
            with_unused_ports,
            network,
            rpc,
            txpool,
            builder,
            debug,
            db,
            dev,
            pruning,
            ..
        } = self;
        PoaNodeCommand {
            datadir,
            config,
            chain,
            federation_mode,
            metrics,
            instance,
            with_unused_ports,
            network,
            rpc,
            txpool,
            builder,
            debug,
            db,
            dev,
            pruning,
            ext,
        }
    }

    /// Execute `poa` command
    pub async fn execute(&self, ctx: CliContext) -> eyre::Result<()>
where {
        tracing::info!(target: "reth::cli", version = ?version::SHORT_VERSION, "Starting reth");

        let Self {
            datadir,
            config,
            chain,
            federation_mode,
            metrics,
            instance,
            with_unused_ports,
            network,
            rpc,
            txpool,
            builder,
            debug,
            db,
            dev,
            pruning,
            ext,
        } = self;

        // set up node config
        let mut node_config = NodeConfig {
            config: config.clone(),
            chain: chain.clone(),
            federation_mode: *federation_mode,
            metrics: metrics.clone(),
            instance: instance.clone(),
            network: network.clone(),
            rpc: rpc.clone(),
            txpool: txpool.clone(),
            builder: builder.clone(),
            debug: debug.clone(),
            db: db.clone(),
            dev: dev.clone(),
            pruning: pruning.clone(),
        };

        // Register the prometheus recorder before creating the database,
        // because database init needs it to register metrics.
        let prometheus_handle = node_config.install_prometheus_recorder()?;

        let data_dir = datadir.unwrap_or_chain_default(node_config.chain.chain);
        let db_path = data_dir.db_path();
        let executor = ctx.task_executor;

        tracing::info!(target: "reth::cli", path = ?db_path, "Opening database");
        let database = Arc::new(init_db(db_path.clone(), self.db.database_args())?.with_metrics());

        if *with_unused_ports {
            node_config = node_config.with_unused_ports();
        }

        // Raise the fd limit of the process.
        // Does not do anything on windows.
        raise_fd_limit()?;

        // async task that checks system clock is in sync with NTP server
        executor.spawn_critical(
            "async system clock sync with ntp task",
            Box::pin(async {
                let sleep_sec = tokio::time::Duration::from_secs(15);
                let acceptable_drift_sec = 1;
                loop {
                    // TODO (scott) pass in ntp url as arg
                    match ntp_unix_timestamp("time.cloudflare.com").await {
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
        // extract the jwt secret from the args if possible
        let default_jwt_path = data_dir.jwt_path();
        let jwt_secret = node_config.rpc.auth_jwt_secret(default_jwt_path)?;

        // This determines which tasks are spawned. For example, the block production and
        // frost tasks are only spawned for a federation node.
        let is_fed_node = node_config.federation_mode;

        // Connect to btc signining server if in federation mode
        let btc_server_client = if is_fed_node {
            let fut = || async {
                let client = BtcServerExtendedClient::new(
                    node_config.rpc.btc_server.clone().expect("btc_server exists"),
                    Some(jwt_secret.clone()),
                )
                .await;
                client
            };

            let client = match retry_exec(fut, 3, Duration::from_secs(2)).await {
                Ok(client) => client,
                Err(err) => {
                    error!(target: "reth::cli", "Failed to connect to btc server: {}", err);
                    return Err(eyre::eyre!("Failed to connect to btc server: {}", err));
                }
            };
            info!(target: "reth::cli", "Btc server connected");

            Some(client)
        } else {
            None
        };

        let bitcoin_block_headers: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>> =
            Arc::new(RwLock::new(None));
        let bitcoin_block_headers_clone = bitcoin_block_headers.clone();

        // create bitcoind client and make sure its synced
        let bitcoind_config: BitcoindConfig = node_config.rpc.bitcoind.clone().into();
        let bitcoind_client =
            BitcoindClient::new(bitcoind_config.clone()).expect("Unable to create bitcoind client");

        info!(target: "reth::cli", "Waiting for bitcoind client to sync...");
        match tokio::time::timeout(Duration::from_secs(60), bitcoind_client.wait_until_synced())
            .await
        {
            Ok(_) => {
                info!(target: "reth::cli", "Bitcoind client synced");
            }
            Err(_) => {
                error!(target: "reth::cli", "Bitcoind client could not achieve synced status within 60 secs. Exiting...");
                return Err(eyre::eyre!(
                    "Bitcoind client could not achieve synced status within 60secs. Exiting..."
                ));
            }
        }

        let bitcoind_config_clone = bitcoind_config.clone();
        executor.spawn_critical(
            "async bitcoin block header task",
            Box::pin(async move {
                /// Sleep interval between wake-ups.
                const SLEEP: tokio::time::Duration = tokio::time::Duration::from_millis(10);

                macro_rules! or_continue {
                    ($e:expr) => {{
                        match $e {
                            Ok(r) => r,
                            Err(_) => {
                                error!(
                                    target: "reth::cli",
                                    "Error calling '{}'. Retrying...",
                                    stringify!($e),
                                );
                                tokio::time::sleep(SLEEP).await;
                                continue;
                            }
                        }
                    }};
                }

                let bitcoind = BitcoindClient::new(bitcoind_config_clone)
                    .expect("Unable to create bitcoind client");
                let mut last_tip = bitcoin::BlockHash::all_zeros();
                loop {
                    let tip_hash = or_continue!(bitcoind.get_best_block_hash());
                    if last_tip != tip_hash {
                        info!("Async bitcoin worker tip changed (new={})", tip_hash);

                        let tip_block = or_continue!(bitcoind.get_block_info(&tip_hash));
                        let height = tip_block.height;
                        let finalized = {
                            let depth =
                                reth_primitives::constants::MAINNET_PEGIN_CONFIRMATION_DEPTH;
                            let height = height.saturating_sub(depth as usize - 1);
                            let hash = or_continue!(bitcoind.get_block_hash(height as u64));
                            or_continue!(bitcoind.get_block_info(&hash))
                        };
                        let header = or_continue!(bitcoind.get_block_header(finalized.hash));

                        *bitcoin_block_headers.write().await =
                            Some((header, finalized.height as u32));
                        last_tip = tip_hash;
                    }
                    tokio::time::sleep(SLEEP).await;
                }
            }),
        );
        info!(target: "reth::cli", "Spawned async bitcoin block header task");

        let static_file_provider = StaticFileProvider::new(data_dir.static_files_path())?;

        let provider_factory = ProviderFactory::<Arc<DatabaseEnv>>::new(
            database.clone(),
            node_config.chain.clone(),
            data_dir.static_files_path(),
        )?;

        // Configure static file producer
        let static_file_producer = StaticFileProducer::new(
            provider_factory.clone(),
            static_file_provider.clone(),
            PruneModes::default(),
        );

        node_config
            .start_metrics_endpoint(
                prometheus_handle,
                Arc::clone(&database),
                static_file_provider.clone(),
                executor.clone(),
            )
            .await?;

        // Load reth config which is a bit different than cli config
        let mut reth_config = self.load_config()?;

        let network_secret_path =
            self.network.p2p_secret_key.clone().unwrap_or_else(|| data_dir.p2p_secret_path());

        debug!(target: "reth::cli", ?network_secret_path, "Loading p2p key file");
        let secret_key = get_secret_key(&network_secret_path)?;

        // add trusted nodes with --trusted-peers flag
        info!(target: "reth::cli", "Adding trusted nodes");
        if !node_config.network.trusted_peers.is_empty() {
            node_config.network.trusted_peers.iter().for_each(|peer| {
                reth_config.peers.trusted_nodes.insert(*peer);
            });
        }

        // add trusted nodes (federation members) with chain.toml
        // assumes chain.toml is present at data_dir
        let chain_path = match PathBuf::from_str(format!("{}/chain.toml", data_dir).as_str()) {
            Ok(path) => path,
            Err(_) => {
                error!(target: "reth::cli", "Failed to create path to chain.toml");
                return Err(eyre::eyre!("Failed to create path to chain.toml"));
            }
        };
        let authorities =
            get_federation_pks_from_path(&chain_path).expect("federation keys to exist");
        self.add_trusted_peers_from_authorities(secret_key, authorities, &mut reth_config);

        let genesis_hash = init_genesis(provider_factory.clone())?;

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        executor.spawn_critical("stages metrics listener task", sync_metrics_listener);

        // Config executor factory
        let evm_config = EthEvmConfig::default();
        let executor_factory = EvmProcessorFactory::new(self.chain.clone(), evm_config.clone());

        // Authority consensus
        let consensus = self.consensus();

        // configure blockchain tree
        let tree_externals = TreeExternals::new(
            provider_factory.clone(),
            consensus.clone(),
            executor_factory.clone(),
        );

        let tree = BlockchainTree::new(
            tree_externals,
            BlockchainTreeConfig::default(),
            None, /* Prune mode */
        )?;

        let canon_state_notification_sender = tree.canon_state_notification_sender();
        let blockchain_tree = Arc::new(ShareableBlockchainTree::new(tree));
        debug!(target: "reth::cli", "configured blockchain tree");

        // fetch the head block from the database
        let head = self.lookup_head(provider_factory.clone());

        // setup the blockchain provider
        let blockchain_db =
            BlockchainProvider::new(provider_factory.clone(), blockchain_tree.clone())?;

        let blob_store = InMemoryBlobStore::default();
        let validator = TransactionValidationTaskExecutor::eth_builder(Arc::clone(&self.chain))
            .with_head_timestamp(head.timestamp)
            .kzg_settings(self.kzg_settings()?)
            .with_additional_tasks(1)
            .build_with_tasks(blockchain_db.clone(), executor.clone(), blob_store.clone());

        // Set up Transaction pool (mempool)
        let transaction_pool =
            reth_transaction_pool::Pool::eth_pool(validator, blob_store, self.txpool.pool_config());
        info!(target: "reth::cli", "Transaction pool initialized");

        // spawn txpool maintenance task
        {
            let pool = transaction_pool.clone();
            let chain_events = blockchain_db.canonical_state_stream();
            let client = blockchain_db.clone();
            executor.spawn_critical(
                "txpool maintenance task",
                reth_transaction_pool::maintain::maintain_transaction_pool_future(
                    client,
                    pool,
                    chain_events,
                    executor.clone(),
                    Default::default(),
                ),
            );
            debug!(target: "reth::cli", "Spawned txpool maintenance task");
        }

        // Set up block import structures
        let (block_import_tx, block_import_rx) = unbounded_channel();
        let block_import = ProofOfAuthorityBlockImport::new(self.chain.clone(), block_import_tx);

        // create frost config if in federation mode
        let frost_config = if is_fed_node {
            // create authority config
            let (authority_index, authorities, authority_pk) = get_authority_signer_index(
                blockchain_db.clone(),
                Arc::clone(&self.chain),
                secp256k1::Secp256k1::new(),
                secret_key,
            )
            .expect("Failed to get authority index");
            let config = FrostConfig::new(
                authority_pk,
                authority_index,
                authorities,
                node_config.rpc.min_signers.expect("min signers"),
                node_config.rpc.max_signers.expect("max signers"),
            );
            info!(target: "reth::cli", "Frost config initialized");

            Some(config)
        } else {
            None
        };

        let default_peers_path = data_dir.known_peers_path();
        let cfg_builder = self
            .network
            .network_config(
                &reth_config,
                self.chain.clone(),
                secret_key.clone(),
                default_peers_path,
            )
            .with_task_executor(Box::new(executor.clone()))
            .set_head(head)
            .listener_addr(SocketAddr::new(
                self.network.addr,
                // set discovery port based on instance number
                self.network.port + self.instance - 1,
            ))
            .discovery_addr(SocketAddr::new(
                self.network.addr,
                // set discovery port based on instance number
                self.network.port + self.instance - 1,
            ))
            .block_import(Box::new(block_import.clone()))
            .frost_config(frost_config.clone())
            .network_mode(reth_network::config::NetworkMode::Authority);

        let network_config = cfg_builder.build(provider_factory.clone());

        // Now we need to build the network components including frost p2p, txpool p2p, eth request
        // handling p2p, as well as the general p2p network
        let (network_handle, network_manager, tx_pool_p2p, eth_request_handler_p2p, frost_p2p) =
            NetworkManager::builder(network_config)
                .await?
                .frost(frost_config.clone())
                .request_handler(provider_factory.clone())
                .transactions(transaction_pool.clone(), Default::default())
                .split_with_handle();
        // Start all the p2p tasks
        let frost_handle = if is_fed_node {
            let frost_manager = frost_p2p.expect("should be some");
            let frost_handle = frost_manager.handle();
            executor.spawn_critical("p2p frost", frost_manager);

            Some(frost_handle)
        } else {
            None
        };
        executor.spawn_critical("txpool p2p task", tx_pool_p2p);
        executor.spawn_critical("eth request handler p2p task", eth_request_handler_p2p);
        executor.spawn_critical("network p2p", network_manager);

        let network_client = network_handle.fetch_client().await?;

        debug!(target: "reth::cli", "Spawning payload builder service");
        let payload_builder = reth_ethereum_payload_builder::EthereumPayloadBuilder::default();
        let conf = DefaultPoAPayloadBuilderConfig {};

        let payload_job_config = BasicPayloadJobGeneratorConfig::default()
            .interval(conf.interval())
            .deadline(conf.deadline())
            .max_payload_tasks(conf.max_payload_tasks())
            .extradata(conf.extradata_bytes())
            .max_gas_limit(conf.max_gas_limit());

        let payload_generator = BasicPayloadJobGenerator::with_builder(
            blockchain_db.clone(),
            transaction_pool.clone(),
            executor.clone(),
            payload_job_config,
            node_config.chain.clone(),
            payload_builder,
        );
        let (payload_service, payload_builder) = PayloadBuilderService::new(
            payload_generator,
            blockchain_db.clone().canonical_state_stream(),
        );

        executor.spawn_critical("payload builder service", Box::pin(payload_service));
        debug!(target: "reth::cli", "Spawned payload builder service");

        // needed for on_node_started
        let components = RethNodeComponents {
            executor: executor.clone(),
            db: blockchain_db.clone(),
            network: network_handle.clone(),
        };
        let (consensus_engine_tx, consensus_engine_rx) = unbounded_channel();
        // Build authority Consensus
        let (
            _,
            block_production_task,
            mut block_fetcher_task,
            frost_task,
            mut sync_controller,
            mut pbft_task,
        ) = AuthorityConsensusBuilder::try_new(
            Arc::clone(&self.chain),
            blockchain_db.clone(),
            consensus_engine_tx.clone(),
            canon_state_notification_sender.clone(),
            btc_server_client.clone(),
            bitcoin_block_headers_clone,
            bitcoind_config,
            secp256k1::Secp256k1::new(),
            secret_key,
            network_handle.clone(),
            network_client.clone(),
            frost_handle,
            block_import_rx,
            executor.clone(),
            evm_config,
            frost_config,
            payload_builder.clone(),
            node_config.rpc.btc_network,
        )
        .expect("Failed to create authority consensus builder")
        .build();

        // TODO do we need this?
        // if let Some(store_path) = self.config.debug.engine_api_store.clone() {
        //     let (engine_intercept_tx, engine_intercept_rx) = unbounded_channel();
        //     let engine_api_store = EngineApiStore::new(store_path);
        //     executor.spawn_critical(
        //         "engine api interceptor",
        //         engine_api_store.intercept(consensus_engine_rx, engine_intercept_tx),
        //     );
        //     consensus_engine_rx = engine_intercept_rx;
        // };

        // configure exxes manager
        let exex_manager = ExExManagerHandle::empty();

        // Configure pipeline
        let max_block = node_config.max_block(&network_client, provider_factory.clone()).await?;
        let mut pipeline = build_networked_pipeline(
            &node_config,
            &StageConfig::default(),
            network_client.clone(),
            Arc::clone(&consensus),
            provider_factory.clone(),
            &executor,
            sync_metrics_tx,
            node_config.prune_config(),
            max_block,
            static_file_producer.clone(),
            evm_config,
            exex_manager,
        )
        .await?;

        let pipeline_events = pipeline.events();

        // spawn a network sync task
        let network_handle_clone = network_handle.clone();
        let (reporting_channels_tx, mut reporting_channels_rx) =
            tokio::sync::mpsc::unbounded_channel::<tokio::sync::oneshot::Sender<LiveSyncStatus>>();
        executor.spawn_critical(
            "Live Sync Task",
            Box::pin(async move {
                while let Some(rx) = reporting_channels_rx.recv().await {
                    match network_handle_clone.is_syncing() {
                        true => {
                            let _ = rx.send(LiveSyncStatus::Syncing);
                        }
                        false => {
                            let _ = rx.send(LiveSyncStatus::Synced);
                        }
                    }
                }
            }),
        );

        // wait until the node is fully synced with the rest of the network
        let start = Instant::now();
        loop {
            let (tx, rx) = tokio::sync::oneshot::channel::<LiveSyncStatus>();
            let _ = reporting_channels_tx.send(tx);

            match rx.await {
                Ok(LiveSyncStatus::Synced) => {
                    info!(target: "reth::cli", "Live sync was successfull! Spawning network services...");
                    break;
                }
                Ok(LiveSyncStatus::Syncing) => {
                    error!(target: "reth::cli", "Syncing... ({} secs taken)", start.elapsed().as_secs());
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                Err(e) => {
                    error!(target: "reth::cli", "Live Sync Error: {:?}", e);
                    return Err(eyre::eyre!("Live Sync receiver error. Exiting..."));
                }
            }
        }

        // Spawn authority consensus specific tasks
        // federation mode tasks
        if is_fed_node {
            executor.spawn_critical(
                "PoA Block Production Task",
                Box::pin(async move {
                    block_production_task.expect("block production task exists").start_task().await;
                }),
            );

            executor.spawn_critical(
                "Frost Task",
                Box::pin(async move {
                    frost_task.expect("frost task exists").start_task().await;
                }),
            );

            executor.spawn_critical(
                "Pbft Task",
                Box::pin(async move {
                    pbft_task.expect("pbft task exists").start_task().await;
                }),
            );
        }

        executor.spawn_critical(
            "PoA Block Fetcher Task",
            Box::pin(async move {
                block_fetcher_task.start_task().await;
            }),
        );
        executor.spawn_critical(
            "PoA Block Sync Controller Task",
            Box::pin(async move {
                sync_controller.start_task().await;
            }),
        );

        let initial_target = node_config.initial_pipeline_target(genesis_hash);
        let hooks = EngineHooks::new();

        // TODO do we want pruner
        //  let pruner_events = if let Some(prune_config) = prune_config {
        //     let mut pruner = PrunerBuilder::new(prune_config.clone())
        //         .max_reorg_depth(tree_config.max_reorg_depth() as usize)
        //         .prune_delete_limit(self.config.chain.prune_delete_limit)
        //         .build(provider_factory, snapshotter.highest_snapshot_receiver());

        //     let events = pruner.events();
        //     hooks.add(PruneHook::new(pruner, Box::new(executor.clone())));

        //     info!(target: "reth::cli", ?prune_config, "Pruner initialized");
        //     Either::Left(events)
        // } else {
        //     Either::Right(stream::empty())
        // };

        // Configure the consensus engine
        let (beacon_consensus_engine, beacon_engine_handle) = BeaconConsensusEngine::with_channel(
            network_client,
            pipeline,
            blockchain_db.clone(),
            Box::new(executor.clone()),
            Box::new(network_handle.clone()),
            max_block,
            node_config.debug.continuous,
            payload_builder.clone(),
            initial_target,
            MIN_BLOCKS_FOR_PIPELINE_RUN,
            consensus_engine_tx,
            consensus_engine_rx,
            hooks,
        )?;
        info!(target: "reth::cli", "Consensus engine initialized");

        let events = stream_select!(
            network_handle.event_listener().map(Into::into),
            beacon_engine_handle.event_listener().map(Into::into),
            pipeline_events.map(Into::into),
            // TODO do we need this?
            // if self.config.debug.tip.is_none() {
            //     Either::Left(
            //         ConsensusLayerHealthEvents::new(Box::new(blockchain_db.clone()))
            //             .map(Into::into),
            //     )
            // } else {
            //     Either::Right(stream::empty())
            // },
            // pruner_events.map(Into::into)
        );
        executor.spawn_critical(
            "events task",
            handle_events(
                Some(network_handle.clone()),
                Some(head.number),
                events,
                database.clone(),
            ),
        );

        let _engine_api = EngineApi::new(
            blockchain_db.clone(),
            self.chain.clone(),
            beacon_engine_handle,
            payload_builder.into(),
            Box::new(executor.clone()),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // adjust rpc port numbers based on instance number
        node_config.adjust_instance_ports();

        // Start RPC servers
        let _rpc_server_handles = node_config
            .rpc
            .start_rpc_server(
                blockchain_db.clone(),
                transaction_pool.clone(),
                network_handle.clone(),
                executor.clone(),
                blockchain_db.clone(),
                evm_config.clone(),
            )
            .await?;

        // TODO do we need start auth server?

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        executor.spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

        let _ = ext.on_node_started(components);

        match rx.await? {
            Ok(()) => info!("Beacon consensus engine exited successfully"),
            Err(error) => {
                error!(target: "reth::cli", %error, "Beacon consensus engine exited with an error")
            }
        };

        Ok(())
    }

    /// Loads the reth config with the given datadir root
    fn load_config(&self) -> eyre::Result<Config> {
        match <std::option::Option<PathBuf> as Clone>::clone(&self.config) {
            Some(config_path) => {
                let mut config = confy::load_path::<Config>(&config_path)
                    .wrap_err_with(|| format!("Could not load config file {:?}", config_path))?;

                info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

                // Update the config with the command line arguments
                config.peers.trusted_nodes_only = self.network.trusted_only;

                if !self.network.trusted_peers.is_empty() {
                    info!(target: "reth::cli", "Adding trusted nodes");
                    self.network.trusted_peers.iter().for_each(|peer| {
                        config.peers.trusted_nodes.insert(*peer);
                    });
                }
                return Ok(config);
            }
            None => return Ok(Config::default()),
        }
    }

    /// Loads `MAINNET_KZG_TRUSTED_SETUP`.
    /// TODO I dont think we need this for PoA
    fn kzg_settings(&self) -> eyre::Result<Arc<KzgSettings>> {
        Ok(Arc::clone(&MAINNET_KZG_TRUSTED_SETUP))
    }

    /// Fetches the head block from the database.
    ///
    /// If the database is empty, returns the genesis block.
    fn lookup_head<DB: Database>(&self, provider: ProviderFactory<DB>) -> Head {
        let provider = provider.provider().expect("provider factory failed");

        let head = provider
            .get_stage_checkpoint(StageId::Finish)
            .expect("get stage point")
            .unwrap_or_default()
            .block_number;

        let header = provider
            .header_by_number(head)
            .expect("missing header by number, database corrupt")
            .expect("the header for the latest block is missing, database is corrupt");

        let total_difficulty = provider
            .header_td_by_number(head)
            .expect("missing header by number, database corrupt")
            .expect("the total difficulty for the latest block is missing, database is corrupt");

        let hash = provider
            .block_hash(head)
            .expect("is some")
            .expect("the hash for the latest block is missing, database is corrupt");

        Head {
            number: head,
            hash,
            difficulty: header.difficulty,
            total_difficulty,
            timestamp: header.timestamp,
        }
    }
    /// Returns the [Consensus] instance to use.
    pub fn consensus(&self) -> Arc<dyn Consensus> {
        Arc::new(AuthorityConsensus::new(self.chain.clone()))
    }

    fn add_trusted_peers_from_authorities(
        &self,
        secret_key: SecretKey,
        authorities: Vec<(PublicKey, SocketAddr)>,
        config: &mut Config,
    ) {
        let self_peer_id = pk2id(&secret_key.public_key(SECP256K1));
        for authority in authorities.iter() {
            // don't add self
            let peer_id = pk2id(&authority.0);
            if self_peer_id != peer_id {
                let peer = NodeRecord::new(authority.1, peer_id);
                config.peers.trusted_nodes.insert(peer);
            }
        }
    }
}

/// Default PoA payload builder config
struct DefaultPoAPayloadBuilderConfig {}
impl PayloadBuilderConfig for DefaultPoAPayloadBuilderConfig {
    fn interval(&self) -> Duration {
        Duration::from_secs(1)
    }

    fn deadline(&self) -> Duration {
        Duration::from_secs(5)
    }

    fn max_payload_tasks(&self) -> usize {
        2
    }

    fn extradata(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn extradata_bytes(&self) -> Bytes {
        Bytes::new()
    }

    fn max_gas_limit(&self) -> u64 {
        ETHEREUM_BLOCK_GAS_LIMIT
    }
}

// *** Botanix specific
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

#[cfg(test)]
mod tests {
    use crate::cli::ext::NoArgs;

    use super::*;
    use reth_discv4::DEFAULT_DISCOVERY_PORT;
    use std::{
        net::{IpAddr, Ipv4Addr},
        path::Path,
    };

    #[test]
    fn parse_help_node_command() {
        let err = PoaNodeCommand::try_parse_args_from(["reth", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    // #[test]
    // fn parse_common_node_command_chain_args() {
    //     for chain in SUPPORTED_CHAINS {
    //         let args: PoaNodeCommand = PoaNodeCommand::<()>::parse_from(["reth", "--chain",
    // chain]);         assert_eq!(args.chain.chain,
    // chain.parse::<reth_primitives::Chain>().unwrap());     }
    // }

    #[test]
    fn parse_discovery_addr() {
        let cmd =
            PoaNodeCommand::try_parse_args_from(["reth", "--discovery.addr", "127.0.0.1"]).unwrap();
        assert_eq!(cmd.network.discovery.addr, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn parse_addr() {
        let cmd = PoaNodeCommand::try_parse_args_from([
            "reth",
            "--discovery.addr",
            "127.0.0.1",
            "--addr",
            "127.0.0.1",
        ])
        .unwrap();
        assert_eq!(cmd.network.discovery.addr, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(cmd.network.addr, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn parse_discovery_port() {
        let cmd = PoaNodeCommand::try_parse_args_from(["reth", "--discovery.port", "300"]).unwrap();
        assert_eq!(cmd.network.discovery.port, 300);
    }

    #[test]
    fn parse_port() {
        let cmd = PoaNodeCommand::try_parse_args_from([
            "reth",
            "--discovery.port",
            "300",
            "--port",
            "99",
        ])
        .unwrap();
        assert_eq!(cmd.network.discovery.port, 300);
        assert_eq!(cmd.network.port, 99);
    }

    #[test]
    fn parse_metrics_port() {
        let cmd = PoaNodeCommand::try_parse_args_from(["reth", "--metrics", "9001"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));

        let cmd = PoaNodeCommand::try_parse_args_from(["reth", "--metrics", ":9001"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));

        let cmd =
            PoaNodeCommand::try_parse_args_from(["reth", "--metrics", "localhost:9001"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));
    }

    #[test]
    fn parse_config_path() {
        let cmd = PoaNodeCommand::try_parse_args_from(["reth", "--config", "my/path/to/reth.toml"])
            .unwrap();
        // always store reth.toml in the data dir, not the chain specific data dir
        let data_dir = cmd.datadir.unwrap_or_chain_default(cmd.chain.chain);
        let config_path = cmd.config.unwrap_or_else(|| data_dir.config_path());
        assert_eq!(config_path, Path::new("my/path/to/reth.toml"));

        let cmd = PoaNodeCommand::try_parse_args_from(["reth"]).unwrap();

        // always store reth.toml in the data dir, not the chain specific data dir
        let data_dir = cmd.datadir.unwrap_or_chain_default(cmd.chain.chain);
        let config_path = cmd.config.clone().unwrap_or_else(|| data_dir.config_path());
        let end = format!("reth/{}/reth.toml", SUPPORTED_CHAINS[0]);
        assert!(config_path.ends_with(end), "{:?}", cmd.config);
    }

    #[test]
    fn parse_db_path() {
        let cmd = PoaNodeCommand::try_parse_args_from(["reth"]).unwrap();
        let data_dir = cmd.datadir.unwrap_or_chain_default(cmd.chain.chain);
        let db_path = data_dir.db_path();
        let end = format!("reth/{}/db", SUPPORTED_CHAINS[0]);
        assert!(db_path.ends_with(end), "{:?}", cmd.config);

        let cmd =
            PoaNodeCommand::try_parse_args_from(["reth", "--datadir", "my/custom/path"]).unwrap();
        let data_dir = cmd.datadir.unwrap_or_chain_default(cmd.chain.chain);
        let db_path = data_dir.db_path();
        assert_eq!(db_path, Path::new("my/custom/path/db"));
    }

    #[test]
    #[cfg(not(feature = "optimism"))] // dev mode not yet supported in op-reth
    fn parse_dev() {
        let cmd = PoaNodeCommand::<NoArgs>::parse_from(["reth", "--dev"]);
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
        let mut cmd = PoaNodeCommand::<NoArgs>::parse_from(["reth"]);
        cmd.rpc.adjust_instance_ports(cmd.instance);
        cmd.network.port = DEFAULT_DISCOVERY_PORT + cmd.instance - 1;
        // check rpc port numbers
        assert_eq!(cmd.rpc.auth_port, 8551);
        assert_eq!(cmd.rpc.http_port, 8545);
        assert_eq!(cmd.rpc.ws_port, 8546);
        // check network listening port number
        assert_eq!(cmd.network.port, 30303);

        let mut cmd = PoaNodeCommand::<NoArgs>::parse_from(["reth", "--instance", "2"]);
        cmd.rpc.adjust_instance_ports(cmd.instance);
        cmd.network.port = DEFAULT_DISCOVERY_PORT + cmd.instance - 1;
        // check rpc port numbers
        assert_eq!(cmd.rpc.auth_port, 8651);
        assert_eq!(cmd.rpc.http_port, 8544);
        assert_eq!(cmd.rpc.ws_port, 8548);
        // check network listening port number
        assert_eq!(cmd.network.port, 30304);

        let mut cmd = PoaNodeCommand::<NoArgs>::parse_from(["reth", "--instance", "3"]);
        cmd.rpc.adjust_instance_ports(cmd.instance);
        cmd.network.port = DEFAULT_DISCOVERY_PORT + cmd.instance - 1;
        // check rpc port numbers
        assert_eq!(cmd.rpc.auth_port, 8751);
        assert_eq!(cmd.rpc.http_port, 8543);
        assert_eq!(cmd.rpc.ws_port, 8550);
        // check network listening port number
        assert_eq!(cmd.network.port, 30305);
    }

    #[test]
    fn parse_with_unused_ports() {
        let cmd = PoaNodeCommand::<NoArgs>::parse_from(["reth", "--with-unused-ports"]);
        assert!(cmd.with_unused_ports);
    }

    #[test]
    fn with_unused_ports_conflicts_with_instance() {
        let err =
            PoaNodeCommand::try_parse_args_from(["reth", "--with-unused-ports", "--instance", "2"])
                .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn with_unused_ports_check_zero() {
        let mut cmd = PoaNodeCommand::<NoArgs>::parse_from(["reth"]);
        cmd.rpc = cmd.rpc.with_unused_ports();
        cmd.network = cmd.network.with_unused_ports();

        // make sure the rpc ports are zero
        assert_eq!(cmd.rpc.auth_port, 0);
        assert_eq!(cmd.rpc.http_port, 0);
        assert_eq!(cmd.rpc.ws_port, 0);

        // make sure the network ports are zero
        assert_eq!(cmd.network.port, 0);
        assert_eq!(cmd.network.discovery.port, 0);

        // make sure the ipc path is not the default
        assert_ne!(cmd.rpc.ipcpath, String::from("/tmp/reth.ipc"));
    }
}
