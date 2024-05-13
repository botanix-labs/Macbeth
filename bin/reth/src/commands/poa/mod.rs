//! Main node command

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, parse_socket_address, SUPPORTED_CHAINS},
        DatabaseArgs, DebugArgs, DevArgs, NetworkArgs, PayloadBuilderArgs, PruningArgs,
        RpcServerArgs, TxPoolArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
};
use bitcoin::hashes::Hash;
use clap::{value_parser, Args, Parser};
use eyre::Context;
use fdlimit::raise_fd_limit;
use reth_authority_consensus::{
    extended_client::BtcServerExtendedClient,
    utils::{get_confirmation_depth, is_testnet},
    AuthorityConsensus,
};
use reth_blockchain_tree::{
    BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals,
};
use reth_btc_wallet::bitcoind::{BitcoindClient, BitcoindConfig};
use reth_cli_runner::CliContext;
use reth_config::Config;
use reth_consensus::Consensus;
use reth_consensus_common::utils;
use reth_db::{database::Database, init_db, DatabaseEnv};
use reth_ethereum_payload_builder::EthereumPayloadBuilder;
use reth_network::{import::ProofOfAuthorityBlockImport, NetworkManager};
use reth_node_builder::{
    components::Components, BuilderContext, NodeBuilder, RethRpcConfig, RethTransactionPoolConfig, WithLaunchContext
};
use reth_node_core::{args::get_secret_key, init::init_genesis, node_config::NodeConfig, version};
use reth_node_ethereum::{EthEngineTypes, EthEvmConfig, EthereumNode};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::{
    constants::eip4844::{LoadKzgSettingsError, MAINNET_KZG_TRUSTED_SETUP},
    kzg::KzgSettings,
    stage::StageId,
    ChainSpec, Head,
};
use reth_provider::{
    providers::BlockchainProvider, BlockHashReader, CanonStateSubscriptions, HeaderProvider,
    ProviderFactory, StageCheckpointReader,
};
use reth_revm::EvmProcessorFactory;
use reth_transaction_pool::{blobstore::InMemoryBlobStore, TransactionValidationTaskExecutor};
use rsntp::AsyncSntpClient;
use std::{
    collections::HashMap,
    ffi::OsString,
    fmt,
    future::Future,
    net::{SocketAddr, SocketAddrV4},
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    sync::{mpsc::unbounded_channel, RwLock},
    time::Duration,
};
use tracing::{debug, error, info};
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

impl<Ext: clap::Args + fmt::Debug> PoaNodeCommand<Ext> {
    /// Replaces the extension of the node command
    pub fn with_ext<E: clap::Args + fmt::Debug>(self, ext: E) -> PoaNodeCommand<E> {
        let Self {
            datadir,
            config,
            chain,
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
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()>
where {
        tracing::info!(target: "reth::cli", version = ?version::SHORT_VERSION, "Starting reth");

        let Self {
            datadir,
            config,
            chain,
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
            config,
            chain,
            metrics,
            instance,
            network,
            rpc,
            txpool,
            builder,
            debug,
            db,
            dev,
            pruning,
        };

        // Register the prometheus recorder before creating the database,
        // because database init needs it to register metrics.
        let prometheus_handle = node_config.install_prometheus_recorder()?;

        let data_dir = datadir.unwrap_or_chain_default(node_config.chain.chain);
        let db_path = data_dir.db_path();
        let executor = ctx.task_executor;

        tracing::info!(target: "reth::cli", path = ?db_path, "Opening database");
        let database = Arc::new(init_db(db_path.clone(), self.db.database_args())?.with_metrics());

        if with_unused_ports {
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
            let client = BtcServerExtendedClient::new(
                node_config.rpc.btc_server.clone(),
                Some(jwt_secret.clone()),
            )
            .await
            .expect("can create btc_server");
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

        // fetch and store bitcoin block tx ids
        let is_testnet = is_testnet(node_config.chain.chain.id());
        let confirmation_depth = get_confirmation_depth(is_testnet);
        let bitcoin_block_tx_ids: Arc<RwLock<HashMap<u64, Vec<bitcoin::Txid>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let bitcoin_block_tx_ids_clone = bitcoin_block_tx_ids.clone();
        let bitcoind_config_clone = bitcoind_config.clone();

        executor.spawn_critical("async bitcoin block tx ids task", Box::pin(async move {
            let sleep_ms = tokio::time::Duration::from_millis(5000);
            let bitcoind_client =  BitcoindClient::new(bitcoind_config_clone).expect("Unable to create bitcoind client");

            let mut current_tx_ids: HashMap<u64, Vec<bitcoin::Txid>> = bitcoin_block_tx_ids.read().await.clone();
            loop {
                let tip = match bitcoind_client.get_tip().await {
                    Ok(tip) => tip,
                    Err(_) => {
                        error!(target: "reth::cli", "Failed to fetch the tip. Retrying...");
                        tokio::time::sleep(sleep_ms).await;
                        continue;
                    }
                };

                // prune tx ids older than confirmation_depth
                let confirmation_depth_u64 = <u32 as Into<u64>>::into(confirmation_depth);
                current_tx_ids.retain(|block_height, _| *block_height >= tip - confirmation_depth_u64);

                let start = tip - confirmation_depth_u64;
                for height in start..=tip {
                    // don't fetch tx ids we already have
                    if !current_tx_ids.is_empty() && current_tx_ids.contains_key(&height) {
                        continue;
                    }
                    let block_hash = match bitcoind_client.get_block_hash(height).await {
                        Ok(block_hash) => block_hash,
                        Err(_) => {
                            error!(target: "reth::cli", "Failed to fetch block hash while fetching tx ids. Retrying...");
                            tokio::time::sleep(sleep_ms).await;
                            break;
                        }
                    };

                    match bitcoind_client.get_block_info(&block_hash).await {
                        Ok(block_info) => {
                            let tx_ids = block_info.tx;
                            current_tx_ids.insert(height, tx_ids);
                        }
                        Err(_) => {
                            error!(target: "reth::cli", "Failed to fetch block info while fetching tx ids. Retrying...");
                            tokio::time::sleep(sleep_ms).await;
                            break;
                        }
                    }
                }
                let mut tx_ids_write = bitcoin_block_tx_ids.write().await;
                *tx_ids_write = current_tx_ids.clone();
                drop(tx_ids_write);
            }
        }));
        info!(target: "reth::cli", "Spawned async bitcoin block tx ids task");

        let bitcoind_config = bitcoind_config.clone();
        executor.spawn_critical(
            "async bitcoin block header task",
            Box::pin(async move {
                let sleep_ms = tokio::time::Duration::from_millis(10);
                let bitcoind_client =  BitcoindClient::new(bitcoind_config).expect("Unable to create bitcoind client");
                let mut current_block_hash = bitcoin::BlockHash::all_zeros();
                loop {
                    let mut header_write = bitcoin_block_headers.write().await;
                    let best_block_hash = match bitcoind_client.get_best_block_hash().await {
                        Ok(current_block_hash) => current_block_hash,
                        Err(_) => {
                            drop(header_write);
                            error!(target: "reth::cli", "Failed to fetch the best block hash. Retrying...");
                            tokio::time::sleep(sleep_ms).await;
                            continue;
                        }
                    };
                    if current_block_hash != best_block_hash {
                        info!("Async bitcoin worker tip mismatch");
                        let block_header: bitcoin::block::Header = match bitcoind_client.get_block_header(best_block_hash).await {
                            Ok(block_header) => block_header,
                            Err(_) => {
                                drop(header_write);
                                error!(target: "reth::cli", "Failed to fetch a block header. Retrying...");
                                tokio::time::sleep(sleep_ms).await;
                                continue;
                            }
                        };

                        let tip = match bitcoind_client.get_tip().await {
                            Ok(block_header) => block_header,
                            Err(_) => {
                                drop(header_write);
                                error!(target: "reth::cli", "Failed to fetch best tip. Retrying...");
                                tokio::time::sleep(sleep_ms).await;
                                continue;
                            }
                        };
                        // TODO (armins) in v1 we will need the nth deep block header not tip
                        *header_write = Some((block_header, tip.try_into().expect("valid conversion")));
                        drop(header_write);
                        current_block_hash = best_block_hash;
                    }
                    tokio::time::sleep(sleep_ms).await;
                }
            }),
        );
        info!(target: "reth::cli", "Spawned async bitcoin block header task");
        let consensus = self.consensus();

        let mut provider_factory = ProviderFactory::new(
            Arc::clone(&database),
            Arc::clone(&node_config.chain),
            static_file,
        )
        .expect("make provider factory");

        // configure snapshotter
        // let snapshotter = reth_snapshot::Snapshotter::new(
        //     provider_factory.clone(),
        //     data_dir.snapshots_path(),
        //     node_config.chain.snapshot_block_interval,
        // )?;

        // provider_factory = provider_factory
        //     .with_snapshots(data_dir.snapshots_path(), snapshotter.highest_snapshot_receiver())?;

        node_config
            .start_metrics_endpoint(
                prometheus_handle,
                Arc::clone(&database),
                static_file,
                executor.clone(),
            )
            .await?;

        /// TODO Need to add trusted peers somehow ?
        // if !node_config.network.trusted_peers.is_empty() {
        //     info!(target: "reth::cli", "Adding trusted nodes");
        //     node_config.network.trusted_peers.iter().for_each(|peer| {
        //         node_config.peers.trusted_nodes.insert(*peer);
        //     });
        // }
        let genesis_hash = init_genesis(provider_factory.clone())?;
        // Note: this should be PoA consenusus only
        // let consensus = self.config.consensus();

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        executor.spawn_critical("stages metrics listener task", sync_metrics_listener);

        // Config executor factory
        let evm_config = EthEvmConfig::default();
        let executor_factory = EvmProcessorFactory::new(self.chain.clone(), evm_config.clone());

        // configure blockchain tree
        let tree_config = BlockchainTreeConfig::default();
        let block_chain_tree_config = BlockchainTreeConfig::default();
        let tree_externals =
            TreeExternals::new(provider_factory.clone(), consensus, executor_factory);
        let blockchain_tree = BlockchainTree::new(tree_externals, block_chain_tree_config, None)?;

        let canon_state_notification_sender = blockchain_tree.canon_state_notification_sender();
        let blockchain_tree = ShareableBlockchainTree::new(blockchain_tree);
        debug!(target: "reth::cli", "configured blockchain tree");

        // fetch the head block from the database
        let head = self.lookup_head(provider_factory);

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

        // Load reth config which is a bit different than cli config
        let reth_config = self.load_config()?;

        info!(target: "reth::cli", "Connecting to P2P network");
        let network_secret_path =
            self.network.p2p_secret_key.clone().unwrap_or_else(|| data_dir.p2p_secret_path());

        debug!(target: "reth::cli", ?network_secret_path, "Loading p2p key file");
        let secret_key = get_secret_key(&network_secret_path)?;

        // Set up block import structures
        let (block_import_tx, block_import_rx) = unbounded_channel();
        let block_import = ProofOfAuthorityBlockImport::new(self.chain.clone(), block_import_tx);

        let default_peers_path = data_dir.known_peers_path();
        let cfg_builder = self
            .network
            .network_config(&reth_config, self.chain.clone(), secret_key, default_peers_path)
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
            .block_import(Box::new(block_import))
            .network_mode(reth_network::config::NetworkMode::Authority);

        let network_config = cfg_builder.build(provider_factory.clone());
        let network_client = network_config.client.clone();
        let mut network_builder = NetworkManager::builder(network_config).await?;

        // TODO Configure payload builder
        let builder_context = BuilderContext::new(head, provider_factory.clone(), executor.clone(), data_dir.clone(), node_config.clone(), reth_config.clone());

        // let eth_payload_builder = PayloadB;
        let payload_builder: PayloadBuilderHandle<EthEngineTypes> =
            
        // TODO config components
        // let components = Components {
        //     transaction_pool: transaction_pool.clone(),
        //     evm_config: evm_config.clone(),
        //     network: network_builder.handle(),
        //     payload_builder,
        // };

        // TODO build authority consensus
        // TODO add back in subprotocols?
        // TODO Start authority specific tasks
        // TODO spawn events task
        // TODO start rpc server
        
        // TODO start beacond engine
        // TODO return node handle?



        
        // do not exit
        loop {};
    }

    /// Loads the reth config with the given datadir root
    fn load_config(&self) -> eyre::Result<Config> {
        let config_path = self.config.expect("is some");
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

        Ok(config)
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

/// No Additional arguments
#[derive(Debug, Clone, Copy, Default, Args)]
#[non_exhaustive]
pub struct NoArgs;

#[cfg(test)]
mod tests {
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

    #[test]
    fn parse_common_node_command_chain_args() {
        for chain in SUPPORTED_CHAINS {
            let args: PoaNodeCommand =
                PoaNodeCommand::<NoArgs>::parse_from(["reth", "--chain", chain]);
            assert_eq!(args.chain.chain, chain.parse::<reth_primitives::Chain>().unwrap());
        }
    }

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
