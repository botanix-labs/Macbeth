//! Contains types and methods that can be used to launch a node running poa consensus based off of
//! a [NodeConfig].

use crate::{
    args::get_secret_key,
    commands::debug_cmd::engine_api_store::EngineApiStore,
    core::{
        events::{cl::ConsensusLayerHealthEvents, node::handle_events},
        init::init_genesis,
    },
};
use bitcoin::hashes::Hash;
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::{future::Either, stream, stream_select, StreamExt};
use reth_authority_consensus::{
    extended_client::BtcServerExtendedClient,
    utils::{get_confirmation_depth, is_testnet},
    AuthorityConsensusBuilder,
};
use reth_beacon_consensus::{
    hooks::{EngineHooks, PruneHook},
    BeaconConsensusEngine, BeaconConsensusEngineError, MIN_BLOCKS_FOR_PIPELINE_RUN,
};
use reth_blockchain_tree::{config::BlockchainTreeConfig, ShareableBlockchainTree};
use reth_config::Config;
use reth_consensus_common::utils::{self, get_authority_signer_index};
use reth_db::{
    database::Database,
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
};
use reth_network::{
    frost::manager::FrostConfig, import::ProofOfAuthorityBlockImport, NetworkEvents,
};
use reth_network_api::{NetworkInfo, PeersInfo};
use reth_node_core::{
    cli::{
        components::{RethNodeComponentsImpl, RethRpcServerHandles},
        config::RethRpcConfig,
        db_type::DatabaseInstance,
        ext::{DefaultRethNodeCommandConfig, RethCliExt, RethNodeCommandConfig},
    },
    dirs::{ChainPath, DataDirPath},
    version::SHORT_VERSION,
};
#[cfg(not(feature = "optimism"))]
use reth_node_ethereum::{EthEngineTypes, EthEvmConfig};
#[cfg(feature = "optimism")]
use reth_node_optimism::{OptimismEngineTypes, OptimismEvmConfig};
use reth_payload_builder::PayloadBuilderHandle;
use reth_provider::{providers::BlockchainProvider, ProviderFactory};
use reth_prune::PrunerBuilder;
use reth_rpc_engine_api::EngineApi;
use reth_tasks::{TaskExecutor, TaskManager};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::{mpsc::unbounded_channel, oneshot, RwLock};
use tracing::*;

use reth_btc_wallet::bitcoind::{BitcoindClient, BitcoindConfig};
use rsntp::AsyncSntpClient;
use tokio::time::Duration;

/// Re-export `NodeConfig` from `reth_node_core`.
pub use reth_node_core::node_config::NodeConfig;

/// Launches the PoA node, also adding any RPC extensions passed.
///
/// # Example
/// ```rust
/// # use reth_tasks::{TaskManager, TaskSpawner};
/// # use reth_node_core::node_config::NodeConfig;
/// # use reth_node_core::cli::{
/// #     ext::DefaultRethNodeCommandConfig,
/// # };
/// # use tokio::runtime::Handle;
/// # use reth::builder::launch_from_config;
///
/// async fn t() {
///     let handle = Handle::current();
///     let manager = TaskManager::new(handle);
///     let executor = manager.executor();
///     let builder = NodeConfig::default();
///     let ext = DefaultRethNodeCommandConfig::default();
///     let handle = launch_from_config::<()>(builder, ext, executor).await.unwrap();
/// }
/// ```
pub async fn launch_poa_from_config<E: RethCliExt>(
    mut config: NodeConfig,
    ext: E::Node,
    executor: TaskExecutor,
) -> eyre::Result<NodeHandle> {
    info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

    let database = std::mem::take(&mut config.database);
    let db_instance = database.init_db(config.db.log_level, config.chain.chain)?;

    match db_instance {
        DatabaseInstance::Real { db, data_dir } => {
            let builder = NodeBuilderWithDatabase { config, db, data_dir };
            builder.launch::<E>(ext, executor).await
        }
        DatabaseInstance::Test { db, data_dir } => {
            let builder = NodeBuilderWithDatabase { config, db, data_dir };
            builder.launch::<E>(ext, executor).await
        }
    }
}

// PBFT consensus allows for at least block to be reorged
const POA_MAX_REORG_DEPTH: u64 = 1;

/// A version of the [NodeConfig] that has an installed database. This is used to construct the
/// [NodeHandle].
///
/// This also contains a path to a data dir that cannot be changed.
#[derive(Debug)]
pub struct NodeBuilderWithDatabase<DB> {
    /// The node config
    pub config: NodeConfig,
    /// The database
    pub db: Arc<DB>,
    /// The data dir
    pub data_dir: ChainPath<DataDirPath>,
}

impl<DB: Database + DatabaseMetrics + DatabaseMetadata + 'static> NodeBuilderWithDatabase<DB> {
    /// Launch the node with the given extensions and executor
    pub async fn launch<E: RethCliExt>(
        mut self,
        mut ext: E::Node,
        executor: TaskExecutor,
    ) -> eyre::Result<NodeHandle> {
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
        let default_jwt_path = self.data_dir.jwt_path();
        let jwt_secret = self.config.rpc.auth_jwt_secret(default_jwt_path)?;

        // Connect to btc signining server
        let btc_server_client = BtcServerExtendedClient::new(
            self.config.rpc.btc_server.clone(),
            Some(jwt_secret.clone()),
        )
        .await
        .expect("cannot create btc_server");
        info!(target: "reth::cli", "Btc server connected");

        let bitcoin_block_headers: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>> =
            Arc::new(RwLock::new(None));
        let bitcoin_block_headers_clone = bitcoin_block_headers.clone();

        // create bitcoind client and make sure its synced
        let bitcoind_config: BitcoindConfig = self.config.rpc.bitcoind.clone().into();
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
        let is_testnet = is_testnet(self.config.chain.chain.id());
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

        // get config
        let config = self.load_config()?;

        let prometheus_handle = self.config.install_prometheus_recorder()?;
        info!(target: "reth::cli", "Database opened");

        let mut provider_factory =
            ProviderFactory::new(Arc::clone(&self.db), Arc::clone(&self.config.chain));

        // configure snapshotter
        let snapshotter = reth_snapshot::Snapshotter::new(
            provider_factory.clone(),
            self.data_dir.snapshots_path(),
            self.config.chain.snapshot_block_interval,
        )?;

        provider_factory = provider_factory.with_snapshots(
            self.data_dir.snapshots_path(),
            snapshotter.highest_snapshot_receiver(),
        )?;

        self.config.start_metrics_endpoint(prometheus_handle, Arc::clone(&self.db)).await?;

        debug!(target: "reth::cli", chain=%self.config.chain.chain, genesis=?self.config.chain.genesis_hash(), "Initializing genesis");

        let genesis_hash = init_genesis(Arc::clone(&self.db), self.config.chain.clone())?;

        // Note: this should be PoA consenusus only
        let consensus = self.config.consensus();

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        executor.spawn_critical("stages metrics listener task", sync_metrics_listener);

        let prune_config = self
            .config
            .pruning
            .prune_config(Arc::clone(&self.config.chain))?
            .or(config.prune.clone());

        // TODO: stateful node builder should be able to remove cfgs here
        // NOTE: not needed for PoA but just keeping it in
        #[cfg(feature = "optimism")]
        let evm_config = OptimismEvmConfig::default();

        // The default payload builder is implemented on the unit type.
        #[cfg(not(feature = "optimism"))]
        let evm_config = EthEvmConfig::default();

        // configure blockchain tree
        // Rest of these block chain tree configs are defaults
        let tree_config = BlockchainTreeConfig::new(
            POA_MAX_REORG_DEPTH,
            65,  /* max_blocks_in_chain */
            256, /* num_of_additional_canonical_block_hashes */
            200, /* max_unconnected_blocks */
        );
        let tree = self.config.build_blockchain_tree(
            provider_factory.clone(),
            consensus.clone(),
            prune_config.clone(),
            sync_metrics_tx.clone(),
            tree_config,
            evm_config,
        )?;
        let canon_state_notification_sender = tree.canon_state_notification_sender();
        let blockchain_tree = ShareableBlockchainTree::new(tree);
        debug!(target: "reth::cli", "configured blockchain tree");

        // fetch the head block from the database
        let head = self
            .config
            .lookup_head(provider_factory.clone())
            .wrap_err("the head block is missing")?;

        // setup the blockchain provider
        let blockchain_db =
            BlockchainProvider::new(provider_factory.clone(), blockchain_tree.clone())?;

        // build transaction pool
        let transaction_pool =
            self.config.build_and_spawn_txpool(&blockchain_db, head, &executor, &self.data_dir)?;

        // get node secret key
        let network_sk = get_secret_key(&self.data_dir.p2p_secret_path())?;

        // create authority config
        let (authority_index, authorities, authority_pk) = get_authority_signer_index(
            blockchain_db.clone(),
            Arc::clone(&self.config.chain),
            secp256k1::Secp256k1::new(),
            network_sk,
        )
        .expect("Failed to get authority index");

        // create frost config
        let frost_config = FrostConfig::new(
            authority_pk,
            authority_index,
            authorities,
            self.config.rpc.frost.min_signers,
            self.config.rpc.frost.max_signers,
        );

        // Set up block import structures
        let (block_import_tx, block_import_rx) = unbounded_channel();
        let block_import =
            ProofOfAuthorityBlockImport::new(self.config.chain.clone(), block_import_tx);
        // build network
        let mut network_builder = self
            .config
            .build_network(
                &config,
                provider_factory.clone(),
                executor.clone(),
                head,
                &self.data_dir,
                Some(Box::new(block_import)),
                Some(frost_config.clone()),
            )
            .await?;

        let components = RethNodeComponentsImpl::new(
            blockchain_db.clone(),
            transaction_pool.clone(),
            network_builder.handle(),
            executor.clone(),
            blockchain_db.clone(),
            evm_config,
        );

        // allow network modifications
        ext.configure_network(network_builder.network_mut(), &components)?;

        // launch network
        let (network, frost_handle) = self.config.start_network(
            network_builder,
            &executor,
            transaction_pool.clone(),
            provider_factory.clone(),
            &self.data_dir,
            Some(frost_config.clone()),
        );

        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), enode = %network.local_node_record(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");
        let network_client = network.fetch_client().await?;

        ext.on_components_initialized(&components)?;

        debug!(target: "reth::cli", "Spawning payload builder service");

        // TODO: stateful node builder should handle this in with_payload_builder
        // Optimism's payload builder is implemented on the OptimismPayloadBuilder type.
        #[cfg(feature = "optimism")]
        let payload_builder = reth_optimism_payload_builder::OptimismPayloadBuilder::default()
            .set_compute_pending_block(self.config.builder.compute_pending_block);

        #[cfg(feature = "optimism")]
        let payload_builder: PayloadBuilderHandle<OptimismEngineTypes> =
            ext.spawn_payload_builder_service(&self.config.builder, &components, payload_builder)?;

        // The default payload builder is implemented on the unit type.
        #[cfg(not(feature = "optimism"))]
        let payload_builder = reth_ethereum_payload_builder::EthereumPayloadBuilder::default();

        #[cfg(not(feature = "optimism"))]
        let payload_builder: PayloadBuilderHandle<EthEngineTypes> =
            ext.spawn_payload_builder_service(&self.config.builder, &components, payload_builder)?;

        let (consensus_engine_tx, mut consensus_engine_rx) = unbounded_channel();

        let bitcoind_config = self.config.rpc.bitcoind.clone().into();

        let network_sk = get_secret_key(&self.data_dir.p2p_secret_path())?;
        let (
            _,
            mut block_production_task,
            mut block_fetcher_task,
            mut frost_task,
            mut sync_controller,
            mut pbft_task,
        ) = AuthorityConsensusBuilder::try_new(
            Arc::clone(&self.config.chain),
            blockchain_db.clone(),
            consensus_engine_tx.clone(),
            canon_state_notification_sender.clone(),
            btc_server_client.clone(),
            bitcoin_block_headers_clone,
            bitcoin_block_tx_ids_clone,
            bitcoind_config,
            secp256k1::Secp256k1::new(),
            network_sk,
            None,
            network.clone(),
            network_client.clone(),
            frost_handle.clone(),
            block_import_rx,
            executor.clone(),
            evm_config,
            frost_config,
            payload_builder.clone(),
            self.config.rpc.btc_network,
        )
        .expect("Failed to create authority consensus builder")
        .build();

        if let Some(store_path) = self.config.debug.engine_api_store.clone() {
            let (engine_intercept_tx, engine_intercept_rx) = unbounded_channel();
            let engine_api_store = EngineApiStore::new(store_path);
            executor.spawn_critical(
                "engine api interceptor",
                engine_api_store.intercept(consensus_engine_rx, engine_intercept_tx),
            );
            consensus_engine_rx = engine_intercept_rx;
        };
        let max_block = self.config.max_block(&network_client, provider_factory.clone()).await?;
        let mut pipeline = self
            .config
            .build_networked_pipeline(
                &config.stages,
                network_client.clone(),
                Arc::clone(&consensus),
                provider_factory.clone(),
                &executor.clone(),
                sync_metrics_tx,
                prune_config.clone(),
                max_block,
                evm_config,
            )
            .await?;

        let pipeline_events = pipeline.events();

        // TODO(armins) do we need this?
        // block_production_task.set_pipeline_events(pipeline_events.clone());

        executor.spawn_critical(
            "PoA Block Production Task",
            Box::pin(async move {
                block_production_task.start_task().await;
            }),
        );
        executor.spawn_critical(
            "PoA Block Fetcher Task",
            Box::pin(async move {
                block_fetcher_task.start_task().await;
            }),
        );
        executor.spawn_critical(
            "Frost Task",
            Box::pin(async move {
                frost_task.start_task().await;
            }),
        );
        executor.spawn_critical(
            "Pbft Task",
            Box::pin(async move {
                pbft_task.start_task().await;
            }),
        );
        executor.spawn_critical(
            "PoA Block Sync Controller Task",
            Box::pin(async move {
                sync_controller.start_task().await;
            }),
        );

        let initial_target = self.config.initial_pipeline_target(genesis_hash);
        let mut hooks = EngineHooks::new();

        let pruner_events = if let Some(prune_config) = prune_config {
            let mut pruner = PrunerBuilder::new(prune_config.clone())
                .max_reorg_depth(tree_config.max_reorg_depth() as usize)
                .prune_delete_limit(self.config.chain.prune_delete_limit)
                .build(provider_factory, snapshotter.highest_snapshot_receiver());

            let events = pruner.events();
            hooks.add(PruneHook::new(pruner, Box::new(executor.clone())));

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
            Box::new(executor.clone()),
            Box::new(network.clone()),
            max_block,
            self.config.debug.continuous,
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
            if self.config.debug.tip.is_none() {
                Either::Left(
                    ConsensusLayerHealthEvents::new(Box::new(blockchain_db.clone()))
                        .map(Into::into),
                )
            } else {
                Either::Right(stream::empty())
            },
            pruner_events.map(Into::into)
        );
        executor.spawn_critical(
            "events task",
            handle_events(Some(network.clone()), Some(head.number), events, self.db.clone()),
        );

        let engine_api = EngineApi::new(
            blockchain_db.clone(),
            self.config.chain.clone(),
            beacon_engine_handle,
            payload_builder.into(),
            Box::new(executor.clone()),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // adjust rpc port numbers based on instance number
        self.config.adjust_instance_ports();

        // Start RPC servers
        let rpc_server_handles =
            self.config.rpc.start_servers(&components, engine_api, jwt_secret, &mut ext).await?;

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        executor.spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

        // TODO launch poa specific CL stuff

        ext.on_node_started(&components)?;

        // NOTE: again not really needed for PoA but just leaving this here
        // If `enable_genesis_walkback` is set to true, the rollup client will need to
        // perform the derivation pipeline from genesis, validating the data dir.
        // When set to false, set the finalized, safe, and unsafe head block hashes
        // on the rollup client using a fork choice update. This prevents the rollup
        // client from performing the derivation pipeline from genesis, and instead
        // starts syncing from the current tip in the DB.
        #[cfg(feature = "optimism")]
        if self.config.chain.is_optimism() && !self.config.rollup.enable_genesis_walkback {
            let client = rpc_server_handles.auth.http_client();
            reth_rpc_api::EngineApiClient::<OptimismEngineTypes>::fork_choice_updated_v2(
                &client,
                reth_rpc_types::engine::ForkchoiceState {
                    head_block_hash: head.hash,
                    safe_block_hash: head.hash,
                    finalized_block_hash: head.hash,
                },
                None,
            )
            .await?;
        }

        // construct node handle and return
        let node_handle = NodeHandle {
            rpc_server_handles,
            consensus_engine_rx: rx,
            terminate: self.config.debug.terminate,
        };
        Ok(node_handle)
    }

    /// Returns the path to the config file.
    fn config_path(&self) -> PathBuf {
        self.config.config.clone().unwrap_or_else(|| self.data_dir.config_path())
    }

    /// Loads the reth config with the given datadir root
    fn load_config(&self) -> eyre::Result<Config> {
        let config_path = self.config_path();

        let mut config = confy::load_path::<Config>(&config_path)
            .wrap_err_with(|| format!("Could not load config file {:?}", config_path))?;

        info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

        // Update the config with the command line arguments
        config.peers.connect_trusted_nodes_only = self.config.network.trusted_only;

        if !self.config.network.trusted_peers.is_empty() {
            info!(target: "reth::cli", "Adding trusted nodes");
            self.config.network.trusted_peers.iter().for_each(|peer| {
                config.peers.trusted_nodes.insert(*peer);
            });
        }

        Ok(config)
    }
}

/// The [NodeHandle] contains the [RethRpcServerHandles] returned by the reth initialization
/// process, as well as a method for waiting for the node exit.
#[derive(Debug)]
pub struct NodeHandle {
    /// The handles to the RPC servers
    rpc_server_handles: RethRpcServerHandles,

    /// The receiver half of the channel for the consensus engine.
    /// This can be used to wait for the consensus engine to exit.
    consensus_engine_rx: oneshot::Receiver<Result<(), BeaconConsensusEngineError>>,

    /// Flag indicating whether the node should be terminated after the pipeline sync.
    terminate: bool,
}

impl NodeHandle {
    /// Returns the [RethRpcServerHandles] for this node.
    pub fn rpc_server_handles(&self) -> &RethRpcServerHandles {
        &self.rpc_server_handles
    }

    /// Waits for the node to exit, if it was configured to exit.
    pub async fn wait_for_node_exit(self) -> eyre::Result<()> {
        self.consensus_engine_rx.await??;

        if self.terminate {
            Ok(())
        } else {
            // The pipeline has finished downloading blocks up to `--debug.tip` or
            // `--debug.max-block`. Keep other node components alive for further usage.
            futures::future::pending().await
        }
    }
}

/// A simple function to launch a node with the specified [NodeConfig], spawning tasks on the
/// [TaskExecutor] constructed from [TaskManager::current].
///
/// # Example
/// ```
/// # use reth_node_core::{
/// #     node_config::NodeConfig,
/// #     args::RpcServerArgs,
/// # };
/// # use reth::builder::spawn_node;
/// async fn t() {
///     // Create a node builder with an http rpc server enabled
///     let rpc_args = RpcServerArgs::default().with_http();
///
///     let builder = NodeConfig::test().with_rpc(rpc_args);
///
///     // Spawn the builder, returning a handle to the node
///     let (_handle, _manager) = spawn_node(builder).await.unwrap();
/// }
/// ```
pub async fn spawn_node(config: NodeConfig) -> eyre::Result<(NodeHandle, TaskManager)> {
    let task_manager = TaskManager::current();
    let ext = DefaultRethNodeCommandConfig::default();
    Ok((launch_poa_from_config::<()>(config, ext, task_manager.executor()).await?, task_manager))
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
