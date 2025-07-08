//! POA node command

use bitcoincore_zmq::subscribe_async_wait_handshake;
use btcserverlib::extended_client::{
    BtcServerExtendedApi, BtcServerExtendedClient, GrpcClientFactory,
};
use clap::{value_parser, Parser};
use client::Empty;
use comet_bft_rpc::HttpCometBFTRpcClientFactory;
use core::panic;
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::TryFutureExt;
use reth_authority_consensus::{
    comet_bft::abci::ABCIDriver,
    random_source_provider::RandomSourceProvider,
    snapshot_manager::SnapshotRunnable,
    utils::{is_known_minting_contract, retry_exec},
    wallet_state_sync::WalletStateSync,
    AuthorityConsensus, AuthorityConsensusBuilder,
};
use reth_cli_util::{get_secret_key, parse_ethereum_address, parse_socket_address};
use reth_db_common::init::init_genesis;
use reth_discv4::NodeRecord;
use reth_network_peers::pk2id;
use reth_node_core::{
    args::{DatadirArgs, StateSyncArgs},
    cli::config::BtcServerConfig,
    version::{
        BUILD_PROFILE_NAME, CARGO_PKG_VERSION, VERGEN_BUILD_TIMESTAMP, VERGEN_CARGO_FEATURES,
        VERGEN_CARGO_TARGET_TRIPLE, VERGEN_GIT_SHA,
    },
};
use reth_node_metrics::{
    hooks::Hooks,
    recorder::install_prometheus_recorder,
    server::{MetricServer, MetricServerConfig},
    version::VersionInfo,
};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::{botanix::mint_validation::MINT_CONTRACT_ADDRESS, Address};
use reth_prune::PruneModes;
use reth_rpc_builder::{config::RethRpcServerConfig, RpcModuleBuilder};
use reth_rpc_eth_types::builder::botanix_config::{Botanix, BotanixConfig};
use reth_stages::StageId;
use reth_tasks::TaskExecutor;
use secp256k1::{PublicKey, SecretKey, SECP256K1};
use std::{borrow::Cow, ffi::OsString, fmt, net::SocketAddr, path::PathBuf, sync::Arc};
use tokio_stream::wrappers::ReceiverStream;

use crate::{
    args::{DatabaseArgs, DebugArgs, NetworkArgs, PayloadBuilderArgs, RpcServerArgs, TxPoolArgs},
    cli::NoArgs,
    payload::PayloadBuilderService,
};
use reth_authority_consensus::bitcoin_checkpoint::{
    BitcoinCheckpointsChain, BitcoinCheckpointsChainSynchronizer, BitcoinHashBlockStream,
    DummyHashBlockStream,
};
use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_btc_wallet::bitcoind::{
    BitcoindClientFactory, BitcoindConfig, BitcoindFactory, RpcApiExt,
};
use reth_chainspec::{BOTANIX_MAINNET_CHAIN_ID, BOTANIX_TESTNET_CHAIN_ID};
use reth_cli_runner::CliContext;
use reth_config::{config::StageConfig, Config};
use reth_consensus_common::utils;
use reth_db::{database::Database, init_db, DatabaseEnv};
use reth_exex::ExExManagerHandle;
use reth_network::{
    frost::{manager::FrostConfig, protocol::FrostProtoHandler},
    protocol::IntoRlpxSubProtocol,
    BlockDownloaderProvider, NetworkHandle, NetworkManager,
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
use reth_primitives::{constants::ETHEREUM_BLOCK_GAS_LIMIT, Bytes, Head};
use reth_provider::{
    providers::{BlockchainProvider2, StaticFileProvider},
    BlockHashReader, CanonStateSubscriptions, DatabaseProviderFactory, HeaderProvider,
    ProviderFactory, StageCheckpointReader,
};
use reth_rpc::EthApi;
use reth_static_file::StaticFileProducer;
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, TransactionPoolExt, TransactionValidationTaskExecutor,
};
use rsntp::AsyncSntpClient;
use tokio::{
    sync::{mpsc::unbounded_channel, oneshot},
    time::{timeout, Duration},
};
use tracing::{debug, error, info};

/// Adds a panic hook to log the panic information
pub fn set_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let payload = panic_info.payload();

        #[allow(clippy::manual_map)]
        let payload = if let Some(s) = payload.downcast_ref::<&str>() {
            Some(&**s)
        } else if let Some(s) = payload.downcast_ref::<String>() {
            Some(s.as_str())
        } else {
            None
        };

        let location = panic_info.location().map(|l| l.to_string());

        error!(panic.payload = payload, panic.location = location, "Uncaught panic");

        std::process::exit(1);
    }));
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
    #[command(flatten)]
    pub datadir: DatadirArgs,

    /// The path to the configuration file to use for network properties.
    #[arg(
        long,
        value_name = "NETWORK_CONFIG_FILE",
        env = "RETH_NETWORK_CONFIG_PATH",
        verbatim_doc_comment
    )]
    pub network_config_path: Option<PathBuf>,

    /// Indicates whether we are running in testnet or not.
    #[arg(long, value_name = "IS_TESTNET", env = "RETH_TESTNET")]
    pub is_testnet: bool,

    /// The NTP server url
    #[arg(
        long,
        value_name = "NTP_SERVER",
        env = "RETH_NTP_SERVER",
        default_value = "time.cloudflare.com"
    )]
    pub ntp_server: String,

    /// The path to the configuration file for the federation setup.
    #[arg(
        long,
        value_name = "FEDERATION_CONFIG_FILE",
        env = "RETH_FEDERATION_CONFIG_FILE",
        verbatim_doc_comment
    )]
    pub federation_config_path: PathBuf,

    /// Run in federation mode. Only the nodes in the federation will be able to produce blocks.
    /// Only nodes defined in chain.toml can enable this flag
    #[arg(
        long,
        value_name = "FEDERATION_MODE",
        env = "RETH_FEDERATION_MODE",
        default_value = "false"
    )]
    pub federation_mode: bool,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long, value_name = "SOCKET", env = "RETH_METRICS_ADDRESS", value_parser = parse_socket_address, help_heading = "Metrics")]
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
    /// - `DISCOVERY_PORT`: default + `instance` - 1
    /// - `AUTH_PORT`: default + `instance` * 100 - 100
    /// - `HTTP_RPC_PORT`: default - `instance` + 1
    /// - `WS_RPC_PORT`: default + `instance` * 2 - 2
    #[arg(long, value_name = "INSTANCE", global = true, default_value_t = 1, env="RETH_INSTANCE", value_parser = value_parser!(u16).range(..=200))]
    pub instance: u16,

    /// Sets all ports to unused, allowing the OS to choose random unused ports when sockets are
    /// bound.
    ///
    /// Mutually exclusive with `--instance`.
    #[arg(long, conflicts_with = "instance", env = "RETH_UNUSED_PORTS", global = true)]
    pub with_unused_ports: bool,

    /// All networking related arguments
    #[command(flatten)]
    pub network: NetworkArgs,

    /// All state sync related arguments
    #[command(flatten)]
    pub state_sync: StateSyncArgs,

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
    #[arg(
        long,
        value_name = "BITCOIND_CONFIG_FILE",
        env = "RETH_BITCOIND_CONFIG_PATH",
        verbatim_doc_comment
    )]
    pub bitcoind_config_path: Option<PathBuf>,

    /// Additional cli arguments
    #[command(flatten, next_help_heading = "Extension")]
    pub ext: Ext,

    /// ABCI client host to listen on
    #[arg(long, value_name = "ABCI_HOST", env = "RETH_ABCI_HOST", default_value_t = String::from("0.0.0.0"))]
    pub abci_host: String,

    /// ABCI client port to listen on
    #[arg(long, value_name = "ABCI_PORT", env = "RETH_ABCI_PORT", default_value_t = 26658)]
    pub abci_port: u16,

    /// `CometBFT` RPC Port
    #[arg(
        long,
        value_name = "COMETBFT_RPC_PORT",
        env = "RETH_COMETBFT_RPC_PORT",
        default_value_t = 26657
    )]
    pub cometbft_rpc_port: u16,

    // TODO parse to a better type
    /// `CometBFT` RPC Host
    #[arg(long, value_name = "COMETBFT_RPC_HOST", env = "RETH_COMETBFT_RPC_HOST", default_value_t = String::from("127.0.0.1"))]
    pub cometbft_rpc_host: String,

    /// Block fee recipient address.
    ///
    /// The input should be a hex string with exactly 40 hex characters.
    /// An optional "0x" prefix is allowed.
    #[arg(
        long,
        value_name = "BLOCK_FEE_RECIPIENT_ADDRESS",
        env = "RETH_BLOCK_FEE_RECIPIENT_ADDRESS",
        value_parser = parse_ethereum_address,
    )]
    pub block_fee_recipient_address: Option<Address>,
}

impl PoaNodeCommand {
    /// Parsers only the default CLI arguments
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Parsers only the default [`PoaNodeCommand`] arguments from the given iterator
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
    #[tracing::instrument(skip_all, err)]
    pub async fn execute(&self, ctx: CliContext) -> eyre::Result<()> {
        tracing::info!(target: "reth::cli", version = ?version::SHORT_VERSION, "Starting reth with poa");
        set_panic_hook();

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
            state_sync,
            block_fee_recipient_address,
        } = self;

        // Load reth config which is a bit different than cli config
        let mut reth_config = self.load_config()?;

        // get the botanix chain spec
        let chain = get_chain_from_federation_config(
            self.federation_config_path.clone().to_str().expect("federation config path to exist"),
            *is_testnet,
        )?;
        let chain_arc = Arc::new(chain.clone());

        // check chains match
        match (chain.chain.id(), rpc.btc_network) {
            (BOTANIX_MAINNET_CHAIN_ID, bitcoin::Network::Bitcoin) => {}
            (BOTANIX_TESTNET_CHAIN_ID, _) => {
                // Testnet can be any non-mainnet network for btc
                if rpc.btc_network == bitcoin::Network::Bitcoin {
                    return Err(eyre::eyre!(
                        "Chains mismatch: Botanix is testnet and btc network is not."
                    ));
                }
            }
            _ => {
                return Err(eyre::eyre!(
                    "Chains mismatch: Botanix is mainnet and btc network is not."
                ));
            }
        }

        // set up node config
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
            state_sync: state_sync.clone(),
        };

        let mut bitcoind_config: BitcoindConfig = node_config.rpc.bitcoind.clone().into();
        // prioritize the bitcoind config path from cli args
        if let Some(bitcoind_config_path) = bitcoind_config_path {
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
                btc_signing_server_jwt_secret.map(|s| btcserverlib::jwt::JwtSecret(s.0)),
            );

            let fut = || async { btc_server_factory.build_and_connect().await };

            let mut btc_server_client =
                match retry_exec("btc_server_start", fut, 3, Duration::from_secs(2)).await {
                    Ok(client) => client,
                    Err(err) => {
                        error!(target: "reth::cli", "Failed to connect to btc server: {}", err);
                        return Err(eyre::eyre!("Failed to connect to btc server: {}", err));
                    }
                };
            info!(target: "reth::cli", "Btc server connected");

            // Check our connection to the btc server is authenticated properly
            btc_server_client.health_check(Empty {}).await.map_err(|err| {
                error!(target: "reth::cli", "Failed to authenticate to btc server: {}", err);
                eyre::eyre!("Failed to authenticate to btc server: {}", err)
            })?;
            info!(target: "reth::cli", "Btc server authenticated");

            Some(btc_server_factory)
        } else {
            None
        };

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

        let bitcoin_checkpoints = Arc::new(BitcoinCheckpointsChain::try_new(
            chain.bitcoin_checkpoint_confirmation_depth as usize,
            chain.historical_bitcoin_checkpoints_count,
            chain.weak_bitcoin_checkpoints_count,
        )?);

        let checkpoints_synchronizer = BitcoinCheckpointsChainSynchronizer::new(
            Arc::clone(&bitcoin_checkpoints),
            bitcoind_client,
        );

        // Connect to Bitcoin ZMQ socket to receive new block notifications
        // to synchronize the bitcoin checkpoints chain with the bitcoind node

        let bitcoin_zmq_block_hash_stream: BitcoinHashBlockStream = if let Some(
            zmq_hash_block_address,
        ) =
            &rpc.bitcoind.zmq_hash_block_address
        {
            // Connect to the ZMQ socket for block hash notifications
            // if the zmq hash block address is provided

            // Timeout if we cannot connect to the ZMQ socket after 5 seconds
            let connection_timeout = Duration::from_secs(5);

            match timeout(
                connection_timeout.clone(),
                subscribe_async_wait_handshake(&[zmq_hash_block_address.as_str()]),
            )
            .await
            {
                Ok(Ok(stream)) => {
                    info!(target: "reth::cli", "Connected to bitcoind ZMQ hashblock socket {}", zmq_hash_block_address);

                    Box::new(stream)
                }
                Ok(Err(err)) => {
                    // Ok from `timeout` but an error from the subscribe function.
                    return Err(eyre::eyre!(
                        "Failed to subscribe to bitcoind ZMQ hashblock socket {}: {}",
                        zmq_hash_block_address,
                        err
                    ));
                }
                Err(_) => {
                    // Timeout error
                    return Err(eyre::eyre!(
                        "Timeout to subscribe to bitcoind ZMQ hashblock socket {} after {} secs",
                        zmq_hash_block_address,
                        connection_timeout.as_secs_f64(),
                    ));
                }
            }
        } else {
            // ZMQ socket for block hash notifications
            // is not provided. Fall back to an interval update logic

            // TODO: Remove this fallback and make zmq socket mandatory when we release
            //  version 2

            let update_interval = Duration::from_secs(5);

            tracing::warn!(target: "reth::cli", "No ZMQ hash block address provided. Using dummy block hash stream with checkpoints update interval of {} seconds", update_interval.as_secs_f64());

            let stream = DummyHashBlockStream::new(update_interval);

            Box::new(stream)
        };

        // Synchronize the local bitcoin checkpoints chain with the bitcoind node

        executor.spawn_critical(
            "async bitcoin checkpoint chain synchronization task",
            checkpoints_synchronizer.sync(bitcoin_zmq_block_hash_stream),
        );

        info!(target: "reth::cli", "Spawned async bitcoin task for block headers");

        let static_file_provider = StaticFileProvider::read_write(data_dir.static_files())?;
        let provider_factory = ProviderFactory::<Arc<DatabaseEnv>>::new(
            database.clone(),
            node_config.chain.clone(),
            static_file_provider.clone(),
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
        let authority_pk = secret_key.public_key(SECP256K1);
        tracing::info!("Federation Member PubKey {:?}", authority_pk.to_string());
        tracing::info!("Federation Member Enode {:?}", pk2id(&authority_pk));

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

        if let Some(max_signers) = rpc.max_signers {
            if federation_authorities.len() != max_signers as usize {
                return Err(eyre::eyre!(
                    "max_signers does not match the length of federation_authorities"
                ));
            }
        }
        self.add_trusted_peers_from_authorities(
            secret_key,
            federation_authorities.clone(),
            &mut reth_config,
        );
        let genesis_authorities =
            federation_authorities.iter().map(|authority| authority.0).collect::<Vec<PublicKey>>();
        let authorities_socket_addresses =
            federation_authorities.iter().map(|authority| authority.1).collect::<Vec<SocketAddr>>();

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        executor.spawn_critical("stages metrics listener task", sync_metrics_listener);

        // Config executor factory
        let evm_config = EthEvmConfig::default();
        let executor_factory = EthExecutorProvider::new(
            Arc::new(chain.clone()),
            evm_config,
            bitcoind_factory.clone(),
            node_config.rpc.btc_network,
            Arc::new(provider_factory.database_provider_ro()?),
        );

        // fetch the head block from the database
        let head = self.lookup_head(provider_factory.clone());
        let latest_sealed_header = provider_factory
            .header(&head.hash)
            .expect("latest block to exist")
            .expect("latest block to exist")
            .seal(head.hash);
        info!(target: "reth::cli", "Latest sealed header: {}", latest_sealed_header.number);

        // Authority consensus
        let consensus = Arc::new(AuthorityConsensus::new(Arc::new(chain)));
        let state_provider = provider_factory.latest().expect("provider factory to exist");
        let blockchain_db =
            BlockchainProvider2::with_latest(provider_factory.clone(), latest_sealed_header)
                .expect("blockchain db to exist");

        let (driver_tx, driver_rx) = tokio::sync::mpsc::channel(1);
        let mut abci_driver =
            ABCIDriver::new(driver_rx, provider_factory.clone(), blockchain_db.clone());

        // check Minting.sol deployed bytecode matches known bytecode
        info!(target: "reth::cli", "Checking minting contract bytecode");
        let deployed_bytecode = state_provider
            .account_code(*MINT_CONTRACT_ADDRESS)
            .expect("Minting contract address exists")
            .expect("Minting contract bytecode to exist");
        if let Err(e) = is_known_minting_contract(
            federation_config.minting_contract_bytecode,
            deployed_bytecode.bytecode(),
        ) {
            error!(target: "reth::cli", "{}", e);
            panic!("{}", e);
        }
        drop(state_provider);

        let blob_store = InMemoryBlobStore::default();
        let validator =
            TransactionValidationTaskExecutor::eth_builder(Arc::clone(&chain_arc.clone()))
                .with_head_timestamp(head.timestamp)
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
            executor.spawn_critical(
                "txpool maintenance task",
                reth_transaction_pool::maintain::maintain_transaction_pool_future(
                    blockchain_db.clone(),
                    pool,
                    chain_events,
                    executor.clone(),
                    Default::default(),
                ),
            );
            debug!(target: "reth::cli", "Spawned txpool maintenance task");
        }

        if let (Some(min_signers), Some(max_signers)) = (rpc.min_signers, rpc.max_signers) {
            if min_signers > max_signers {
                return Err(eyre::eyre!("min_signers should be less than or equal to max_signers"));
            }
        }
        // create frost config if in federation mode
        let frost_config = if is_fed_node {
            let authority_index =
                genesis_authorities.iter().position(|a| a == &authority_pk).ok_or_else(|| {
                    eyre::eyre!(
                        "Your public key could not be found in the list of federation public keys"
                    )
                })?;

            let config = FrostConfig::new(
                authority_pk,
                authority_index,
                genesis_authorities.clone(),
                node_config
                    .rpc
                    .min_signers
                    .ok_or_else(|| eyre::eyre!("min signers not specified"))?,
                node_config
                    .rpc
                    .max_signers
                    .ok_or_else(|| eyre::eyre!("max signers not specified"))?,
                node_config.state_sync.wallet_state_sync_chunk_size,
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
            .disable_discovery()
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
            .network_mode(reth_network::config::NetworkMode::Authority);

        // Frost sub protocol is only supported by federation nodes
        if is_fed_node {
            let (protocol_events_tx, protocol_events_rx) = tokio::sync::mpsc::channel(10_000);
            let my_peer_id = pk2id(&secret_key.public_key(SECP256K1));
            let protocol_handler = FrostProtoHandler { my_peer_id, protocol_events_tx };

            network_cfg_builder = network_cfg_builder
                .frost_protocol_events_rx(ReceiverStream::new(protocol_events_rx))
                .add_rlpx_sub_protocol(protocol_handler.into_rlpx_sub_protocol());
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
        let (payload_service, payload_builder) =
            PayloadBuilderService::new(payload_generator, blockchain_db.canonical_state_stream());

        executor.spawn_critical("payload builder service", Box::pin(payload_service));
        debug!(target: "reth::cli", "Spawned payload builder service");

        let cometbft_rpc_factory = HttpCometBFTRpcClientFactory::default()
            .with_port(*cometbft_rpc_port)
            .with_host(cometbft_rpc_host);

        // Build authority Consensus
        let (abci_started_tx, abci_started_rx) = tokio::sync::oneshot::channel::<()>();
        let bitcoind_client = bitcoind_factory.build_and_connect().expect("bitcoind client");
        let (frost_task, abci_client_builder, snapshot_manager, wallet_sync) =
            match AuthorityConsensusBuilder::try_new(
                Arc::clone(&chain_arc.clone()),
                blockchain_db.clone(),
                btc_server_factory.clone(),
                bitcoin_checkpoints.clone(),
                secret_key,
                network_handle.clone(),
                frost_handle,
                executor.clone(),
                frost_config,
                node_config.rpc.btc_network,
                genesis_authorities.clone(),
                authorities_socket_addresses,
                executor_factory.clone(),
                bitcoind_factory.clone(),
                evm_config,
                cometbft_rpc_factory,
                RandomSourceProvider::new(),
                driver_tx,
                node_config.clone().state_sync,
                provider_factory.clone(),
                *block_fee_recipient_address,
                bitcoind_client,
            ) {
                Ok(consensus) => consensus.build::<BtcServerExtendedClient>().await,
                Err(e) => {
                    return Err(eyre::eyre!("AuthorityConsensusBuilderError : {:?}", e));
                }
            };

        if let Some(mut snapshot_manager) = snapshot_manager {
            tracing::info!("Snapshot manager is enabled.");
            executor.spawn_critical(
                "Snapshot Manager",
                Box::pin(async move {
                    if let Err(e) = snapshot_manager.run().await {
                        error!(target: "reth::cli", "Snapshot Manager Error: {:?}", e);
                    }
                }),
            );
        }

        if let Some(wallet_sync) = wallet_sync {
            executor.spawn_critical(
                "Wallet Sync",
                Box::pin(async move {
                    if let Err(e) = wallet_sync.sync_wallet_state().await {
                        error!(target: "reth::cli", "Wallet Sync Error: {:?}", e);
                    }
                }),
            );
        }

        // configure exxes manager
        let exex_manager = ExExManagerHandle::empty();

        // Configure pipeline
        let max_block = node_config.max_block(&network_client, provider_factory.clone()).await?;
        build_networked_pipeline(
            &StageConfig::default(),
            network_client.clone(),
            Arc::new(consensus.clone()),
            provider_factory,
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

        // Spawn authority consensus specific tasks
        // federation mode tasks
        // TODO  we should structure which tasks are spawned based on the node type using two
        // different structs
        if is_fed_node {
            executor.spawn_critical(
                "Frost Task",
                Box::pin(async move {
                    frost_task.expect("frost task exists").start_task(abci_started_rx).await;
                }),
            );
        }

        // NOTE: the node will block here until DKG has completed
        let abci_client_builder = abci_client_builder.expect("abci client builder exists");
        let fut = || async {
            abci_client_builder
                .start_server(
                    &executor.clone(),
                    transaction_pool.clone(),
                    abci_host.to_string(),
                    *abci_port,
                )
                .await
        };

        match retry_exec("abci_server_start", fut, 3, Duration::from_secs(2)).await {
            Ok(()) => {}
            Err(err) => {
                error!(target: "reth::cli", "Failed to connect to abci client: {}", err);
                return Err(eyre::eyre!("Failed to connect to abci client: {}", err));
            }
        };

        // adjust rpc port numbers based on instance number
        node_config.adjust_instance_ports();

        // create botanix client
        let botanix_config =
            BotanixConfig::new(node_config.rpc.btc_network, bitcoind_factory.clone());

        // Start RPC servers
        let botanix_provider = Botanix::new(botanix_config);
        let node_components = PoaNodeComponents::new(
            transaction_pool.clone(),
            evm_config,
            executor_factory.clone(),
            network_handle.clone(),
            blockchain_db.clone(),
            payload_builder.clone(),
            executor.clone(),
        );

        // add metrics if necessary
        if let Some(metrics_listener_address) = metrics {
            // start the metrics server
            info!(target: "reth::cli", "Starting metrics endpoint at {}", metrics_listener_address.to_string());
            let config = MetricServerConfig::new(
                *metrics_listener_address,
                VersionInfo {
                    version: CARGO_PKG_VERSION,
                    build_timestamp: VERGEN_BUILD_TIMESTAMP,
                    cargo_features: VERGEN_CARGO_FEATURES,
                    git_sha: VERGEN_GIT_SHA,
                    target_triple: VERGEN_CARGO_TARGET_TRIPLE,
                    build_profile: BUILD_PROFILE_NAME,
                },
                executor.clone(),
                Hooks::new(database.clone(), static_file_provider),
            );
            MetricServer::new(config).serve().await?;
        }

        let _rpc_handle = {
            let module_config = self.rpc.transport_rpc_module_config();
            let rpc_modules = RpcModuleBuilder::default()
                .with_provider(node_components.provider.clone())
                .with_pool(node_components.pool.clone())
                .with_network(node_components.network.clone())
                .with_events(blockchain_db.clone())
                .with_executor(node_components.task_executor.clone())
                .with_evm_config(node_components.evm_config)
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
        abci_started_tx.send(()).expect("abci started tx");

        let (tx, rx) = oneshot::channel();
        executor.spawn_critical(
            "abci driver",
            Box::pin(async move {
                let res = abci_driver.start().await;
                let _ = tx.send(res);
            }),
        );

        match rx.await? {
            Ok(()) => info!("ABCIDriver exited successfully"),
            Err(error) => {
                error!(target: "reth::cli", %error, "ABCIDriver exited with an error")
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
        for authority in &authorities {
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
    #[allow(dead_code)]
    /// evm executor factory
    pub executor: EthExecutorProvider<BitcoindClientFactory, Arc<DatabaseEnv>>,
    /// network handle
    pub network: NetworkHandle,
    #[allow(dead_code)]
    /// The blockchain provider
    pub provider: BlockchainProvider2<Arc<DatabaseEnv>>,
    /// payload builder
    pub payload_builder: PayloadBuilderHandle<EthEngineTypes>,
    /// task executor
    pub task_executor: TaskExecutor,
}

impl<P> PoaNodeComponents<P>
where
    P: TransactionPoolExt + 'static,
{
    pub(crate) const fn new(
        pool: P,
        evm_config: EthEvmConfig,
        executor: EthExecutorProvider<BitcoindClientFactory, Arc<DatabaseEnv>>,
        network: NetworkHandle,
        provider: BlockchainProvider2<Arc<DatabaseEnv>>,
        payload_builder: PayloadBuilderHandle<EthEngineTypes>,
        task_executor: TaskExecutor,
    ) -> Self {
        Self { pool, evm_config, executor, network, provider, payload_builder, task_executor }
    }
}

/// Default `PoA` payload builder config
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
        let federation_config = FederationTomlConfig::new(
            authorities,
            "0x".to_string(),
            "0x".to_string(),
            "0x".to_string(),
        );
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
    fn parse_db_path_testnet() {
        let secret_key = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let authority = FedMemberPubKey {
            key: secret_key.public_key(SECP256K1).to_string(),
            socket_addr: format!("127.0.0.1:30303"),
        };
        let authorities = vec![authority];
        let federation_config = FederationTomlConfig::new(
            authorities,
            "0x".to_string(),
            "0x".to_string(),
            "0x".to_string(),
        );
        let cmd = PoaNodeCommand::try_parse_args_from([
            "reth",
            "--datadir",
            "my/custom/path",
            "--federation-config-path",
            "my/path/to/federation.toml",
            "--is-testnet",
        ])
        .unwrap();
        let chain = get_botanix_chain(
            &federation_config.to_string().expect("should parse to string"),
            cmd.is_testnet,
        )
        .expect("chain is to exist");
        assert_eq!(chain.chain.id(), BOTANIX_TESTNET_CHAIN_ID);
        assert_ne!(cmd.rpc.btc_network, bitcoin::Network::Bitcoin);
        let data_dir =
            cmd.datadir.datadir.clone().unwrap_or_chain_default(chain.chain, cmd.datadir);
        let db_path = data_dir.db();
        assert_eq!(db_path, Path::new("my/custom/path/db"));
    }

    #[test]
    fn parse_db_path_mainnet() {
        let secret_key = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let authority = FedMemberPubKey {
            key: secret_key.public_key(SECP256K1).to_string(),
            socket_addr: format!("127.0.0.1:30303"),
        };
        let authorities = vec![authority];
        let federation_config = FederationTomlConfig::new(
            authorities,
            "0x".to_string(),
            "0x".to_string(),
            "0x".to_string(),
        );
        let cmd = PoaNodeCommand::try_parse_args_from([
            "reth",
            "--datadir",
            "my/custom/path",
            "--federation-config-path",
            "my/path/to/federation.toml",
            "--btc-network",
            "bitcoin",
        ])
        .unwrap();
        let chain = get_botanix_chain(
            &federation_config.to_string().expect("should parse to string"),
            cmd.is_testnet,
        )
        .expect("chain is to exist");
        assert_eq!(chain.chain.id(), BOTANIX_MAINNET_CHAIN_ID);
        assert_eq!(cmd.rpc.btc_network, bitcoin::Network::Bitcoin);
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
