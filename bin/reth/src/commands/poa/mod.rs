//! Main node command

use bitcoin::hashes::Hash;
use bitcoincore_rpc::RpcApi;
use btcserverlib::extended_client::GrpcClientFactory;
use clap::{value_parser, Parser};
use client::{Empty, SyncTxIndexRequest};
use comet_bft_rpc::HttpCometBFTRpcClientFactory;
use core::panic;
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::{stream_select, StreamExt, TryFutureExt};
use reth_authority_consensus::{
    utils::{is_known_minting_contract, retry_exec},
    AuthorityConsensus, AuthorityConsensusBuilder, LightCBFTClientBuilder,
};
use reth_cli_commands::node::NoArgs;
use reth_cli_util::{get_secret_key, parse_socket_address};
use reth_db_common::init::init_genesis;
use reth_discv4::NodeRecord;
use reth_engine_util::EngineMessageStreamExt;
use reth_network_peers::pk2id;
use reth_node_core::{
    args::DatadirArgs,
    cli::config::BtcServerConfig,
    version::{CARGO_PKG_VERSION, CLIENT_CODE, NAME_CLIENT, VERGEN_GIT_SHA},
};
use reth_node_metrics::recorder::install_prometheus_recorder;
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::botanix::mint_validation::MINT_CONTRACT_ADDRESS;
use reth_prune::PruneModes;
use reth_rpc_builder::{config::RethRpcServerConfig, RpcModuleBuilder};
use reth_rpc_engine_api::capabilities::EngineCapabilities;
use reth_rpc_eth_types::builder::botanix_config::{Botanix, BotanixConfig};
use reth_rpc_types::engine::ClientVersionV1;
use reth_stages::StageId;
use reth_tasks::TaskExecutor;
use secp256k1::{PublicKey, SecretKey, SECP256K1};
use std::{borrow::Cow, ffi::OsString, fmt, net::SocketAddr, path::PathBuf, sync::Arc};
use tokio_stream::wrappers::UnboundedReceiverStream;

use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_beacon_consensus::{
    hooks::EngineHooks, BeaconConsensusEngine, MIN_BLOCKS_FOR_PIPELINE_RUN,
};
use reth_blockchain_tree::{
    BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals,
};
use reth_btc_wallet::bitcoind::{
    BitcoindClientFactory, BitcoindConfig, BitcoindFactory, RpcApiExt,
};
use reth_cli_runner::CliContext;
use reth_config::{config::StageConfig, Config};
use reth_consensus_common::utils;
use reth_db::{database::Database, init_db, DatabaseEnv};
use reth_exex::ExExManagerHandle;
use reth_network::{
    frost::manager::FrostConfig, import::ProofOfAuthorityBlockImport, BlockDownloaderProvider,
    NetworkEventListenerProvider, NetworkHandle, NetworkManager,
};
use reth_node_builder::{
    setup::build_networked_pipeline, PayloadBuilderConfig, RethTransactionPoolConfig,
};
use reth_node_core::{
    args::{
        utils::{get_chain_from_federation_config, load_federation_config_toml},
        BitcoindArgs,
    },
    node_config::NodeConfig,
    version,
};
use reth_node_ethereum::{EthEngineTypes, EthEvmConfig, EthExecutorProvider};
use reth_node_events::node::handle_events;
use reth_primitives::{constants::ETHEREUM_BLOCK_GAS_LIMIT, Bytes, Head};
use reth_provider::{
    providers::{BlockchainProvider, StaticFileProvider},
    BlockHashReader, CanonStateSubscriptions, HeaderProvider, ProviderFactory,
    StageCheckpointReader,
};
use reth_revm::primitives::EnvKzgSettings;
use reth_rpc::{EngineApi, EthApi};
use reth_static_file::StaticFileProducer;
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, TransactionPoolExt, TransactionValidationTaskExecutor,
};
use rsntp::AsyncSntpClient;
use tokio::{
    sync::{mpsc::unbounded_channel, oneshot, RwLock},
    time::Duration,
};

use tracing::{debug, error, info};

use crate::{
    args::{DatabaseArgs, DebugArgs, NetworkArgs, PayloadBuilderArgs, RpcServerArgs, TxPoolArgs},
    payload::PayloadBuilderService,
};

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
    #[command(flatten)]
    pub datadir: DatadirArgs,

    /// The path to the configuration file to use for network properties.
    #[arg(long, value_name = "NETWORK_CONFIG_FILE", verbatim_doc_comment)]
    pub network_config_path: Option<PathBuf>,

    /// Indicates whether we are running in testnet or not.
    #[arg(long, value_name = "IS_TESTNET", default_value = "true")]
    pub is_testnet: bool,

    /// The NTP server url
    #[arg(long, value_name = "NTP_SERVER", default_value = "time.cloudflare.com")]
    pub ntp_server: String,

    /// The path to the configuration file for the federation setup.
    #[arg(long, value_name = "FEDERATION_CONFIG_FILE", verbatim_doc_comment)]
    pub federation_config_path: PathBuf,

    /// Run in federation mode. Only the nodes in the federation will be able to produce blocks.
    /// Only nodes defined in chain.toml can enable this flag
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
    #[command(flatten)]
    pub network: NetworkArgs,

    /// All rpc related arguments
    #[command(flatten)]
    pub rpc: RpcServerArgs,

    /// All txpool related arguments with --txpool prefix
    #[command(flatten)]
    pub txpool: TxPoolArgs,

    /// All debug related arguments with --debug prefix
    #[command(flatten)]
    pub debug: DebugArgs,

    /// All database related arguments
    #[command(flatten)]
    pub db: DatabaseArgs,

    /// The path to the configuration file to use for network properties.
    #[arg(long, value_name = "BITCOIND_CONFIG_FILE", verbatim_doc_comment)]
    pub bitcoind_config_path: Option<PathBuf>,

    /// Additional cli arguments
    #[command(flatten, next_help_heading = "Extension")]
    pub ext: Ext,

    /// ABCI client host to listen on
    #[arg(long, value_name = "ABCI_HOST", default_value_t = String::from("0.0.0.0"))]
    pub abci_host: String,

    /// ABCI client port to listen on
    #[arg(long, value_name = "ABCI_PORT", default_value_t = 26658)]
    pub abci_port: u16,

    /// CometBFT RPC Port
    #[arg(long, value_name = "COMETBFT_RPC_PORT", default_value_t = 26657)]
    pub cometbft_rpc_port: u16,

    // TODO parse to a better type
    /// CometBFT RPC Host
    #[arg(long, value_name = "COMETBFT_RPC_HOST", default_value_t = String::from("127.0.0.1"))]
    pub cometbft_rpc_host: String,
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
    /// Execute `poa` command
    pub async fn execute(&self, ctx: CliContext) -> eyre::Result<()> {
        tracing::info!(target: "reth::cli", version = ?version::SHORT_VERSION, "Starting reth with poa");

        let Self {
            datadir,
            network_config_path,
            is_testnet,
            ntp_server,
            federation_config_path: _,
            federation_mode,
            metrics,
            instance,
            with_unused_ports,
            network,
            rpc,
            txpool,
            debug,
            db,
            bitcoind_config_path,
            ext: _,
            abci_host,
            abci_port,
            cometbft_rpc_port,
            cometbft_rpc_host,
        } = self;

        // Load reth config which is a bit different than cli config
        let mut reth_config = self.load_config()?;

        // get the botanix chain spec
        let chain = get_chain_from_federation_config(
            self.federation_config_path.clone().to_str().expect("federation config path to exist"),
            *is_testnet,
        )?;
        let chain_arc = Arc::new(chain.clone());

        // set up node config
        // TODO should set up PoaConfig
        let mut node_config = NodeConfig {
            datadir: datadir.clone(),
            config: network_config_path.clone(),
            chain: chain_arc.clone(),
            federation_mode: *federation_mode,
            metrics: *metrics,
            instance: *instance,
            network: network.clone(),
            rpc: rpc.clone(),
            txpool: txpool.clone(),
            debug: debug.clone(),
            db: *db,
            dev: Default::default(),
            pruning: Default::default(),
            builder: PayloadBuilderArgs::default(),
        };

        let mut bitcoind_config: BitcoindConfig = node_config.rpc.bitcoind.clone().into();
        // prioritize the bitcoind config path from cli args
        if let Some(bitcoind_config_path) = bitcoind_config_path {
            // node_config.rpc.bitcoind = Some(bitcoind_config_path);
            let config =
                confy::load_path::<BitcoindArgs>(&bitcoind_config_path).wrap_err_with(|| {
                    format!("Could not load config file {:?}", bitcoind_config_path)
                })?;

            info!(target: "reth::cli", path = ?bitcoind_config_path, "Bitcoind config loaded from file");
            bitcoind_config = config.into();
        }
        let bitcoind_factory: BitcoindClientFactory =
            BitcoindClientFactory::new(bitcoind_config.clone());

        // Register the prometheus recorder before creating the database,
        // because database init needs it to register metrics.
        let _prometheus_handle = install_prometheus_recorder();

        let data_dir =
            datadir.datadir.unwrap_or_chain_default(node_config.chain.chain, datadir.clone());
        let db_path = data_dir.db();
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
        let ntp_server = ntp_server.clone();
        info!("NTP server url: {}", ntp_server);
        executor.spawn_critical(
            "async system clock sync with ntp task",
            Box::pin(async move {
                let sleep_sec = tokio::time::Duration::from_secs(15);
                let acceptable_drift_sec = 1;
                loop {
                    match ntp_unix_timestamp(&ntp_server).await {
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
        // extract the btc server jwt secret from the args
        let btc_signing_server_jwt_secret = node_config.rpc.btc_signing_server_jwt_secret()?;

        // This determines which tasks are spawned. For example, the block production and
        // frost tasks are only spawned for a federation node.
        let is_fed_node = node_config.federation_mode;

        // Connect to btc signining server if in federation mode
        let btc_server_factory = if is_fed_node {
            let btc_server_factory = GrpcClientFactory::new(
                node_config.rpc.btc_server.clone().expect("btc_server exists"),
                btc_signing_server_jwt_secret.clone().map(Into::into),
            );

            let fut = || async { btc_server_factory.build_and_connect().await };

            let mut client = match retry_exec(fut, 3, Duration::from_secs(2)).await {
                Ok(client) => client,
                Err(err) => {
                    error!(target: "reth::cli", "Failed to connect to btc server: {}", err);
                    return Err(eyre::eyre!("Failed to connect to btc server: {}", err));
                }
            };
            info!(target: "reth::cli", "Btc server connected");

            // Check our connection to the btc server is authenticated properly
            client.health_check(Empty {}).await.map_err(|err| {
                error!(target: "reth::cli", "Failed to authenticate to btc server: {}", err);
                eyre::eyre!("Failed to authenticate to btc server: {}", err)
            })?;
            info!(target: "reth::cli", "Btc server authenticated");

            Some(btc_server_factory)
        } else {
            None
        };

        let bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>> =
            Arc::new(RwLock::new(None));
        let bitcoin_block_header_clone = bitcoin_block_header.clone();

        // create bitcoind client and make sure its synced
        let bitcoind_client = bitcoind_factory.build_and_connect().expect("bitcoind client");

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

        let bitcoind_factory_clone = bitcoind_factory.clone();
        let bitcoind_signing_server_factory_clone = btc_server_factory.clone();
        let pegin_conf_depth = chain.parent_confirmation_depth;
        assert_ne!(pegin_conf_depth, 0, "pegin conf depth not set correctly");
        executor.spawn_critical(
            "async bitcoin task for block headers",
            Box::pin(async move {
                /// Sleep interval between wake-ups.
                const SLEEP: tokio::time::Duration = tokio::time::Duration::from_secs(1);

                macro_rules! or_continue {
                    ($e:expr) => {{
                        match $e {
                            Ok(r) => r,
                            Err(_) => {
                                error!(
                                    target: "reth::cli",
                                    "Async bitcoin task error calling '{}'. Retrying...",
                                    stringify!($e),
                                );
                                tokio::time::sleep(SLEEP).await;
                                continue;
                            }
                        }
                    }};
                }

                // Note: we should be panicing if our connection to bitcoind is severed
                let bitcoind =
                    bitcoind_factory_clone.build_and_connect().expect("can connect to bitcoind");
                let mut last_tip = bitcoin::BlockHash::all_zeros();
                loop {
                    let tip_hash = or_continue!(bitcoind.get_best_block_hash());
                    if last_tip != tip_hash {
                        let tip_block = or_continue!(bitcoind.get_block_info(&tip_hash));
                        let height = tip_block.height;
                        let finalized = {
                            let height = height.saturating_sub(pegin_conf_depth as usize);
                            let hash = or_continue!(bitcoind.get_block_hash(height as u64));
                            or_continue!(bitcoind.get_block_info(&hash))
                        };
                        let header = or_continue!(bitcoind.get_block_header(&finalized.hash));

                        info!(
                            "Async bitcoin task setting checkpoint to {}:{}",
                            finalized.height,
                            header.block_hash(),
                        );
                        info!("Async bitcoin Tip {}:{}", height, tip_hash,);
                        *bitcoin_block_header.write().await =
                            Some((header, finalized.height as u32));
                        last_tip = tip_hash;

                        // Sync the bitcoind signing server's txindexer
                        if let Some(btc_factory) = &bitcoind_signing_server_factory_clone {
                            let res = btc_factory.build_and_connect().await;

                            match res {
                                Ok(mut client) => {
                                    let _ = client
                                        .tx_index_new_checkpoint(SyncTxIndexRequest {
                                            checkpoint_block_hash: header
                                                .block_hash()
                                                .to_byte_array()
                                                .to_vec(),
                                        })
                                        .await;
                                }
                                Err(e) => {
                                    error!("Failed to connect to btc signing server: {}", e);
                                }
                            }
                        }
                    }
                    tokio::time::sleep(SLEEP).await;
                }
            }),
        );
        info!(target: "reth::cli", "Spawned async bitcoin task for block headers");

        let static_file_provider = StaticFileProvider::read_write(data_dir.static_files())?;
        let provider_factory = ProviderFactory::<Arc<DatabaseEnv>>::new(
            database.clone(),
            node_config.chain.clone(),
            static_file_provider,
        );

        let genesis_hash = init_genesis(provider_factory.clone())?;
        info!(target: "reth::cli", "Genesis hash: {}", genesis_hash);

        // Configure static file producer
        let static_file_producer =
            StaticFileProducer::new(provider_factory.clone(), PruneModes::default());

        let network_secret_path =
            self.network.p2p_secret_key.clone().unwrap_or_else(|| data_dir.p2p_secret());

        debug!(target: "reth::cli", ?network_secret_path, "Loading p2p key file");
        let secret_key = get_secret_key(&network_secret_path)?;

        // add trusted nodes with --trusted-peers flag
        info!(target: "reth::cli", "Adding trusted nodes");
        if !node_config.network.trusted_peers.is_empty() {
            node_config.network.trusted_peers.iter().for_each(|peer| {
                reth_config.peers.trusted_nodes.push(peer.clone());
            });
        }

        // add trusted nodes (federation members) with federation.toml
        let federation_config = match load_federation_config_toml(&self.federation_config_path) {
            Ok(federation_config) => federation_config,
            Err(_) => {
                error!(target: "reth::cli", "Failed to read federation config file");
                return Err(eyre::eyre!("Failed to read federation config file"));
            }
        };
        let federation_authorities = federation_config.get_federation_pks_from_path()?;
        self.add_trusted_peers_from_authorities(
            secret_key,
            federation_authorities.clone(),
            &mut reth_config,
        );
        let genesis_authorities =
            federation_authorities.iter().map(|authority| authority.0).collect::<Vec<PublicKey>>();
        let authorities_socket_addresses =
            federation_authorities.iter().map(|authority| authority.1).collect::<Vec<SocketAddr>>();

        let authority_pk = secret_key.public_key(SECP256K1);
        tracing::info!("Federation Member PubKey {:?}", authority_pk.to_string());
        tracing::info!("Federation Member Enode {:?}", pk2id(&authority_pk));

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        executor.spawn_critical("stages metrics listener task", sync_metrics_listener);

        // Config executor factory
        let evm_config = EthEvmConfig::default();
        //let executor_factory = EvmFactory::new(Arc::new(chain.clone()), evm_config);
        let executor_factory = EthExecutorProvider::new(
            Arc::new(chain.clone()),
            evm_config,
            bitcoind_factory.clone(),
            node_config.rpc.btc_network,
        );

        // Authority consensus
        let consensus = Arc::new(AuthorityConsensus::new(Arc::new(chain)));

        // configure blockchain tree
        let tree_externals = TreeExternals::new(
            provider_factory.clone(),
            consensus.clone(),
            executor_factory.clone(),
        );

        let tree = BlockchainTree::new(
            tree_externals,
            BlockchainTreeConfig::default(),
            PruneModes::none(), /* Prune mode */
        )?;

        let canon_state_notification_sender = tree.canon_state_notification_sender();
        let blockchain_tree = Arc::new(ShareableBlockchainTree::new(tree));
        debug!(target: "reth::cli", "configured blockchain tree");

        // fetch the head block from the database
        let head = self.lookup_head(provider_factory.clone());

        // setup the blockchain provider
        let blockchain_db =
            BlockchainProvider::new(provider_factory.clone(), blockchain_tree.clone())?;

        // check Minting.sol deployed bytecode matches known bytecode
        info!(target: "reth::cli", "Checking minting contract bytecode");
        let state_provider = provider_factory.latest().expect("provider factory to exist");
        let deployed_bytecode = state_provider
            .account_code(*MINT_CONTRACT_ADDRESS)
            .expect("Minting contract address exists")
            .expect("Minting contract bytecode to exist");
        if let Err(e) = is_known_minting_contract(
            federation_config.minting_contract_bytecode,
            &deployed_bytecode.bytecode(),
        ) {
            error!(target: "reth::cli", "{}", e);
            panic!("{}", e);
        }

        let blob_store = InMemoryBlobStore::default();
        let validator =
            TransactionValidationTaskExecutor::eth_builder(Arc::clone(&chain_arc.clone()))
                .with_head_timestamp(head.timestamp)
                .kzg_settings(self.kzg_settings()?)
                .with_additional_tasks(1)
                .build_with_tasks(blockchain_db.clone(), executor.clone(), blob_store.clone());

        // Set up Transaction pool (mempool)
        let transaction_pool = reth_transaction_pool::Pool::eth_pool(
            validator.clone(),
            blob_store,
            self.txpool.pool_config(),
        );

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
        let block_import = ProofOfAuthorityBlockImport::new(chain_arc.clone(), block_import_tx);

        // create frost config if in federation mode
        let frost_config = if is_fed_node {
            let authority_index =
                genesis_authorities.iter().position(|a| a == &authority_pk).unwrap();
            let config = FrostConfig::new(
                authority_pk,
                authority_index,
                genesis_authorities.clone(),
                node_config.rpc.min_signers.expect("min signers"),
                node_config.rpc.max_signers.expect("max signers"),
            );
            info!(target: "reth::cli", "Frost config initialized");

            Some(config)
        } else {
            None
        };

        let default_peers_path = data_dir.known_peers();
        let mut network_cfg_builder = self
            .network
            .network_config(&reth_config, chain_arc.clone(), secret_key, default_peers_path)
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
            .frost_config(frost_config.clone())
            .network_mode(reth_network::config::NetworkMode::Authority);

        if !is_fed_node {
            // block import is only needed for non-federation nodes
            // federation nodes will recieve blocks via their consensus layer
            network_cfg_builder = network_cfg_builder.block_import(Box::new(block_import.clone()))
        }

        let network_config = network_cfg_builder.build(provider_factory.clone());

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
            .extradata(conf.extradata_bytes());

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

        let (consensus_engine_tx, consensus_engine_rx) = unbounded_channel();

        let consensus_engine_stream = UnboundedReceiverStream::from(consensus_engine_rx)
            .maybe_skip_fcu(node_config.debug.skip_fcu)
            .maybe_skip_new_payload(node_config.debug.skip_new_payload)
            .maybe_reorg(
                blockchain_db.clone(),
                evm_config,
                reth_payload_validator::ExecutionPayloadValidator::new(node_config.chain.clone()),
                node_config.debug.reorg_frequency,
            )
            // Store messages _after_ skipping so that `replay-engine` command
            // would replay only the messages that were observed by the engine
            // during this run.
            .maybe_store_messages(node_config.debug.engine_api_store.clone());

        let cometbft_rpc_factory = HttpCometBFTRpcClientFactory::default()
            .with_port(*cometbft_rpc_port)
            .with_host(cometbft_rpc_host);

        let light_client = {
            if !is_fed_node {
                let light_client = LightCBFTClientBuilder::new(cometbft_rpc_factory.clone())
                    .build_and_verify()
                    .await;

                Some(light_client)
            } else {
                None
            }
        };
        // Build authority Consensus
        let (
            _authority_consensus,
            block_fetcher_task,
            frost_task,
            mut sync_controller,
            _healthcheck_task,
            abci_client_builder,
        ) = AuthorityConsensusBuilder::try_new(
            Arc::clone(&chain_arc.clone()),
            blockchain_db.clone(),
            consensus_engine_tx.clone(),
            canon_state_notification_sender.clone(),
            btc_server_factory.clone(),
            bitcoin_block_header_clone.clone(),
            secret_key,
            network_handle.clone(),
            network_client.clone(),
            frost_handle,
            block_import_rx,
            executor.clone(),
            frost_config,
            payload_builder.clone(),
            node_config.rpc.btc_network,
            genesis_authorities.clone(),
            authorities_socket_addresses,
            executor_factory.clone(),
            bitcoind_factory.clone(),
            evm_config.clone(),
            cometbft_rpc_factory,
            light_client,
        )
        .expect("Failed to create authority consensus builder")
        .build()
        .await;

        // configure exxes manager
        let exex_manager = ExExManagerHandle::empty();

        // Configure pipeline
        let max_block = node_config.max_block(&network_client, provider_factory.clone()).await?;
        let pipeline = build_networked_pipeline(
            &StageConfig::default(),
            network_client.clone(),
            Arc::new(consensus.clone()),
            provider_factory.clone(),
            &executor,
            sync_metrics_tx,
            node_config.prune_config(),
            max_block,
            static_file_producer.clone(),
            executor_factory.clone(),
            exex_manager,
            bitcoind_factory.clone(),
            node_config.rpc.btc_network,
        )?;

        let pipeline_events = pipeline.events();

        // Spawn authority consensus specific tasks
        // federation mode tasks
        // TODO  we should structure which tasks are spawned based on the node type using two
        // different structs
        if is_fed_node {
            executor.spawn_critical(
                "Frost Task",
                Box::pin(async move {
                    frost_task.expect("frost task exists").start_task().await;
                }),
            );

            // executor.spawn_critical(
            //     "Healthcheck Task",
            //     Box::pin(async move {
            //         healthcheck_task.expect("health check task exists").start_task().await;
            //     }),
            // );
        }

        let eth_tx_validator = validator.validator;
        let abci_client_builder = abci_client_builder.expect("abci client builder exists");
        let fut = || async {
            abci_client_builder
                .start_server(
                    &executor.clone(),
                    eth_tx_validator.clone(),
                    transaction_pool.clone(),
                    abci_host.to_string(),
                    *abci_port,
                )
                .await
        };

        match retry_exec(fut, 3, Duration::from_secs(2)).await {
            Ok(()) => {}
            Err(err) => {
                error!(target: "reth::cli", "Failed to connect to abci client: {}", err);
                return Err(eyre::eyre!("Failed to connect to abci client: {}", err));
            }
        };
        if !is_fed_node {
            info!(target: "reth::cli", "Starting PoA Block Fetcher Task");
            executor.spawn_critical(
                "PoA Block Fetcher Task",
                Box::pin(async move {
                    block_fetcher_task.expect("block fetcher task exists").start_task().await;
                }),
            );
            executor.spawn_critical(
                "PoA Block Sync Controller Task",
                Box::pin(async move {
                    sync_controller.start_task().await;
                }),
            );
        }

        let initial_target = node_config.debug.tip;
        let hooks = EngineHooks::new();

        // Configure the consensus engine
        let (beacon_consensus_engine, beacon_engine_handle) = BeaconConsensusEngine::with_channel(
            network_client,
            pipeline,
            blockchain_db.clone(),
            Box::new(executor.clone()),
            Box::new(network_handle.clone()),
            max_block,
            payload_builder.clone(),
            initial_target,
            MIN_BLOCKS_FOR_PIPELINE_RUN,
            consensus_engine_tx,
            Box::pin(consensus_engine_stream),
            hooks,
        )?;
        info!(target: "reth::cli", "Consensus engine initialized");

        let events = stream_select!(
            network_handle.event_listener().map(Into::into),
            beacon_engine_handle.event_listener().map(Into::into),
            pipeline_events.map(Into::into),
        );
        executor.spawn_critical(
            "events task",
            handle_events(Some(Box::new(network_handle.clone())), Some(head.number), events),
        );

        // adjust rpc port numbers based on instance number
        node_config.adjust_instance_ports();

        // build client
        let client = ClientVersionV1 {
            code: CLIENT_CODE,
            name: NAME_CLIENT.to_string(),
            version: CARGO_PKG_VERSION.to_string(),
            commit: VERGEN_GIT_SHA.to_string(),
        };

        // create botanix client
        let botanix_config = BotanixConfig::default()
            .btc_server(node_config.rpc.btc_server.clone())
            .bitcoin_network(node_config.rpc.btc_network)
            .bitcoind(
                bitcoind_config.url().to_owned(),
                bitcoind_config.username().to_owned(),
                bitcoind_config.password().to_owned(),
            )
            .btc_server_jwt_secret(btc_signing_server_jwt_secret.clone().map(Into::into));

        // Start RPC servers
        let botanix_provider = Botanix::new(botanix_config);
        let _engine_api = EngineApi::new(
            blockchain_db.clone(),
            chain_arc.clone(),
            beacon_engine_handle,
            payload_builder.clone().into(),
            Box::new(executor.clone()),
            client,
            EngineCapabilities::default(),
            botanix_provider.clone(),
        );

        // generate deault jwt for the rpc server (as required by reth)
        let default_jwt_path = data_dir.jwt();
        let _reth_auth_jwt_secret = node_config.rpc.auth_jwt_secret(default_jwt_path)?;

        let node_components = PoaNodeComponents::new(
            transaction_pool.clone(),
            evm_config.clone(),
            executor_factory.clone(),
            network_handle.clone(),
            blockchain_db.clone(),
            payload_builder.clone(),
            executor.clone(),
        );

        let _rpc_handle = {
            let module_config = self.rpc.transport_rpc_module_config();
            let rpc_modules = RpcModuleBuilder::default()
                .with_provider(node_components.provider.clone())
                .with_pool(node_components.pool.clone())
                .with_network(node_components.network.clone())
                .with_events(node_components.provider.clone())
                .with_executor(node_components.task_executor.clone())
                .with_evm_config(node_components.evm_config.clone())
                .with_botanix_provider(botanix_provider.clone())
                .build(module_config, Box::new(EthApi::with_spawner));

            let server_config = self.rpc.rpc_server_config();
            let cloned_modules = rpc_modules.clone();
            let launch_rpc = server_config.start(&cloned_modules).map_ok(|handle| {
                if let Some(path) = handle.ipc_endpoint() {
                    info!(target: "reth::cli", %path, "RPC IPC server started");
                }
                if let Some(addr) = handle.http_local_addr() {
                    info!(target: "reth::cli", url=%addr, "RPC HTTP server started");
                }
                if let Some(addr) = handle.ws_local_addr() {
                    info!(target: "reth::cli", url=%addr, "RPC WS server started");
                }
                handle
            });

            launch_rpc.await?
        };

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        executor.spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

        // let _ = ext.on_node_started(components);

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
        match <std::option::Option<PathBuf> as Clone>::clone(&self.network_config_path) {
            Some(config_path) => {
                let mut config = confy::load_path::<Config>(&config_path)
                    .wrap_err_with(|| format!("Could not load config file {:?}", config_path))?;

                info!(target: "reth::cli", path = ?config_path, "Network onfiguration loaded");

                // Update the config with the command line arguments
                config.peers.trusted_nodes_only = self.network.trusted_only;

                if !self.network.trusted_peers.is_empty() {
                    info!(target: "reth::cli", "Adding trusted nodes");
                    self.network.trusted_peers.iter().for_each(|peer| {
                        config.peers.trusted_nodes.push(peer.clone());
                    });
                }
                Ok(config)
            }
            None => Ok(Config::default()),
        }
    }

    /// Loads `MAINNET_KZG_TRUSTED_SETUP`.
    /// TODO I dont think we need this for PoA
    fn kzg_settings(&self) -> eyre::Result<EnvKzgSettings> {
        Ok(EnvKzgSettings::Default)
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
                config.peers.trusted_nodes.push(NodeRecord::new(authority.1, peer_id).into());
            }
        }
    }
}

/// Poa node components needed for the rpc server
#[allow(missing_debug_implementations)]
#[derive(Clone)]
pub struct PoaNodeComponents<P> {
    /// The transaction pool
    pub pool: P,
    /// The EVM config, should always be the default
    pub evm_config: EthEvmConfig,
    /// evm executor factory
    pub executor: EthExecutorProvider<BitcoindClientFactory>,
    /// network handle
    pub network: NetworkHandle,
    /// The blockchain provider
    pub provider: BlockchainProvider<Arc<DatabaseEnv>>,
    /// payload builder
    pub payload_builder: PayloadBuilderHandle<EthEngineTypes>,
    /// task executor
    pub task_executor: TaskExecutor,
}

impl<P> PoaNodeComponents<P>
where
    P: TransactionPoolExt + 'static,
{
    pub(crate) fn new(
        pool: P,
        evm_config: EthEvmConfig,
        executor: EthExecutorProvider<BitcoindClientFactory>,
        network: NetworkHandle,
        provider: BlockchainProvider<Arc<DatabaseEnv>>,
        payload_builder: PayloadBuilderHandle<EthEngineTypes>,
        task_executor: TaskExecutor,
    ) -> Self {
        Self { pool, evm_config, executor, network, provider, payload_builder, task_executor }
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
    use super::*;
    use reth_discv4::DEFAULT_DISCOVERY_PORT;
    use reth_node_core::args::{utils::get_botanix_chain, FedMemberPubKey, FederationTomlConfig};
    use std::{
        net::{IpAddr, Ipv4Addr},
        path::Path,
    };

    use secp256k1::rand;

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
        let cmd = PoaNodeCommand::try_parse_args_from([
            "reth",
            "--discovery.addr",
            "127.0.0.1",
            "--federation-config-path",
            "my/path/to/federation.toml",
        ])
        .unwrap();
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
            "--federation-config-path",
            "my/path/to/federation.toml",
        ])
        .unwrap();
        assert_eq!(cmd.network.discovery.addr, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(cmd.network.addr, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn parse_discovery_port() {
        let cmd = PoaNodeCommand::try_parse_args_from([
            "reth",
            "--discovery.port",
            "300",
            "--federation-config-path",
            "my/path/to/federation.toml",
        ])
        .unwrap();
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
            "--federation-config-path",
            "my/path/to/federation.toml",
        ])
        .unwrap();
        assert_eq!(cmd.network.discovery.port, 300);
        assert_eq!(cmd.network.port, 99);
    }

    #[test]
    fn parse_metrics_port() {
        let cmd = PoaNodeCommand::try_parse_args_from([
            "reth",
            "--metrics",
            "9001",
            "--federation-config-path",
            "my/path/to/federation.toml",
        ])
        .unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));

        let cmd = PoaNodeCommand::try_parse_args_from([
            "reth",
            "--metrics",
            ":9001",
            "--federation-config-path",
            "my/path/to/federation.toml",
        ])
        .unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));

        let cmd = PoaNodeCommand::try_parse_args_from([
            "reth",
            "--metrics",
            "localhost:9001",
            "--federation-config-path",
            "my/path/to/federation.toml",
        ])
        .unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));
    }

    #[test]
    fn parse_config_path() {
        let cmd = PoaNodeCommand::try_parse_args_from([
            "reth",
            "--network-config-path",
            "my/path/to/reth.toml",
            "--federation-config-path",
            "my/path/to/federation.toml",
        ])
        .unwrap();

        let secret_key = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let authority = FedMemberPubKey {
            key: secret_key.public_key(SECP256K1).to_string(),
            socket_addr: format!("127.0.0.1:30303"),
        };
        let authorities = vec![authority];
        let federation_config =
            FederationTomlConfig::new(authorities, "0x".to_string(), "0x".to_string());
        let chain = get_botanix_chain(
            &federation_config.to_string().expect("should parse to string"),
            cmd.is_testnet,
        )
        .expect("chain is to exist");
        // always store reth.toml in the data dir, not the chain specific data dir
        let data_dir =
            cmd.datadir.datadir.clone().unwrap_or_chain_default(chain.chain, cmd.datadir);
        let config_path = cmd.network_config_path.unwrap_or_else(|| data_dir.config());
        assert_eq!(config_path, Path::new("my/path/to/reth.toml"));

        // assert doesn't apply anymore
        // always store reth.toml in the data dir, not the chain specific data dir
        // let data_dir = cmd.datadir.unwrap_or_chain_default(chain.chain);
        // let config_path = cmd.network_config_path.clone().unwrap_or_else(||
        // data_dir.config_path()); let end = format!("reth/{}/reth.toml",
        // SUPPORTED_CHAINS[0]); assert!(config_path.ends_with(end), "{:?}",
        // cmd.network_config_path);
    }

    #[test]
    fn parse_db_path() {
        let cmd = PoaNodeCommand::try_parse_args_from([
            "reth",
            "--network-config-path",
            "my/path/to/reth.toml",
            "--federation-config-path",
            "my/path/to/federation.toml",
        ])
        .unwrap();

        let secret_key = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let authority = FedMemberPubKey {
            key: secret_key.public_key(SECP256K1).to_string(),
            socket_addr: format!("127.0.0.1:30303"),
        };
        let authorities = vec![authority];
        let federation_config =
            FederationTomlConfig::new(authorities, "0x".to_string(), "0x".to_string());
        let cmd = PoaNodeCommand::try_parse_args_from([
            "reth",
            "--datadir",
            "my/custom/path",
            "--federation-config-path",
            "my/path/to/federation.toml",
        ])
        .unwrap();
        let chain = get_botanix_chain(
            &federation_config.to_string().expect("should parse to string"),
            cmd.is_testnet,
        )
        .expect("chain is to exist");
        let data_dir =
            cmd.datadir.datadir.clone().unwrap_or_chain_default(chain.chain, cmd.datadir);
        let db_path = data_dir.db();
        assert_eq!(db_path, Path::new("my/custom/path/db"));
    }

    #[test]
    fn parse_instance() {
        let mut cmd = PoaNodeCommand::<NoArgs>::parse_from([
            "reth",
            "--federation-config-path",
            "my/path/to/federation.toml",
        ]);
        cmd.rpc.adjust_instance_ports(cmd.instance);
        cmd.network.port = DEFAULT_DISCOVERY_PORT + cmd.instance - 1;
        // check rpc port numbers
        assert_eq!(cmd.rpc.auth_port, 8551);
        assert_eq!(cmd.rpc.http_port, 8545);
        assert_eq!(cmd.rpc.ws_port, 8546);
        // check network listening port number
        assert_eq!(cmd.network.port, 30303);

        let mut cmd = PoaNodeCommand::<NoArgs>::parse_from([
            "reth",
            "--instance",
            "2",
            "--federation-config-path",
            "my/path/to/federation.toml",
        ]);
        cmd.rpc.adjust_instance_ports(cmd.instance);
        cmd.network.port = DEFAULT_DISCOVERY_PORT + cmd.instance - 1;
        // check rpc port numbers
        assert_eq!(cmd.rpc.auth_port, 8651);
        assert_eq!(cmd.rpc.http_port, 8544);
        assert_eq!(cmd.rpc.ws_port, 8548);
        // check network listening port number
        assert_eq!(cmd.network.port, 30304);

        let mut cmd = PoaNodeCommand::<NoArgs>::parse_from([
            "reth",
            "--instance",
            "3",
            "--federation-config-path",
            "my/path/to/federation.toml",
        ]);
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
        let cmd = PoaNodeCommand::<NoArgs>::parse_from([
            "reth",
            "--with-unused-ports",
            "--federation-config-path",
            "my/path/to/federation.toml",
        ]);
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
        let mut cmd = PoaNodeCommand::<NoArgs>::parse_from([
            "reth",
            "--federation-config-path",
            "my/path/to/federation.toml",
        ]);
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
