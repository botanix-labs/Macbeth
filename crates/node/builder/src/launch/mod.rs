//! Abstraction for launching a node.

use crate::{
    builder::{NodeAdapter, NodeAddOns, NodeTypesAdapter},
    components::{NodeComponents, NodeComponentsBuilder},
    hooks::NodeHooks,
    node::FullNode,
    BuilderContext, NodeBuilderWithComponents, NodeHandle,
};
use futures::{future, future::Either, stream, stream_select, StreamExt};
use reth_authority_consensus::{
    extended_client::BtcServerExtendedClient,
    utils::{get_confirmation_depth, is_testnet},
    AuthorityConsensusBuilder,
};
use reth_auto_seal_consensus::AutoSealConsensus;
use reth_beacon_consensus::{
    hooks::{EngineHooks, PruneHook, StaticFileHook},
    BeaconConsensus, BeaconConsensusEngine, BeaconEngineMessage,
};
use reth_blockchain_tree::{
    BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals,
};
use reth_consensus_common::utils::{self, get_authority_signer_index};
use reth_exex::{ExExContext, ExExHandle, ExExManager, ExExManagerHandle};
use reth_interfaces::p2p::either::EitherDownloader;
use reth_network::{
    frost::manager::FrostConfig, import::ProofOfAuthorityBlockImport, NetworkEvents,
};
use tokio::sync::mpsc::UnboundedSender;
use reth_consensus::Consensus;
use reth_consensus_common::utils::unix_timestamp;
use reth_node_api::{FullNodeComponents, FullNodeTypes};
use reth_node_core::{
    args::get_secret_key,
    dirs::{ChainPath, DataDirPath},
    engine_api_store::EngineApiStore,
    engine_skip_fcu::EngineApiSkipFcu,
    exit::NodeExitFuture,
};
use reth_node_events::{cl::ConsensusLayerHealthEvents, node};
use reth_primitives::format_ether;
use reth_provider::{providers::BlockchainProvider, CanonStateSubscriptions};
use reth_revm::EvmProcessorFactory;
use reth_rpc_engine_api::EngineApi;
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::{debug, error, info};
use reth_transaction_pool::TransactionPool;
use std::{collections::HashMap, future::Future, sync::Arc};
use tokio::sync::{mpsc::unbounded_channel, oneshot, RwLock};
use tokio::time::Duration;

use reth_btc_wallet::bitcoind::{BitcoindClient, BitcoindConfig};

pub mod common;
pub mod poa;
use bitcoin::hashes::Hash;
pub use common::LaunchContext;
use reth_blockchain_tree::noop::NoopBlockchainTree;

/// A general purpose trait that launches a new node of any kind.
///
/// Acts as a node factory.
///
/// This is essentially the launch logic for a node.
///
/// See also [DefaultNodeLauncher] and [NodeBuilderWithComponents::launch_with]
pub trait LaunchNode<Target> {
    /// The node type that is created.
    type Node;

    /// Create and return a new node asynchronously.
    fn launch_node(self, target: Target) -> impl Future<Output = eyre::Result<Self::Node>> + Send;

    /// Create and return a new poa node asynchronously.
    fn launch_poa_node(
        self,
        target: Target,
    ) -> impl Future<Output = eyre::Result<Self::Node>> + Send;
}

/// The default launcher for a node.
#[derive(Debug)]
pub struct DefaultNodeLauncher {
    /// The task executor for the node.
    pub ctx: LaunchContext,
}

impl DefaultNodeLauncher {
    /// Create a new instance of the default node launcher.
    pub fn new(task_executor: TaskExecutor, data_dir: ChainPath<DataDirPath>) -> Self {
        Self { ctx: LaunchContext::new(task_executor, data_dir) }
    }
}

impl<T, CB> LaunchNode<NodeBuilderWithComponents<T, CB>> for DefaultNodeLauncher
where
    T: FullNodeTypes<Provider = BlockchainProvider<<T as FullNodeTypes>::DB>>,
    CB: NodeComponentsBuilder<T>,
{
    type Node = NodeHandle<NodeAdapter<T, CB::Components>>;

    async fn launch_node(
        self,
        target: NodeBuilderWithComponents<T, CB>,
    ) -> eyre::Result<Self::Node> {
        let Self { ctx } = self;
        let NodeBuilderWithComponents {
            adapter: NodeTypesAdapter { database },
            components_builder,
            add_ons: NodeAddOns { hooks, rpc, exexs: installed_exex },
            config,
        } = target;

        // setup the launch context
        let ctx = ctx
            .with_configured_globals()
            // load the toml config
            .with_loaded_toml_config(config)?
            // attach the database
            .attach(database.clone())
            // ensure certain settings take effect
            .with_adjusted_configs()
            // Create the provider factory
            .with_provider_factory()?
            .inspect(|_| {
                info!(target: "reth::cli", "Database opened");
            })
            .with_prometheus().await?
            .inspect(|this| {
                debug!(target: "reth::cli", chain=%this.chain_id(), genesis=?this.genesis_hash(), "Initializing genesis");
            })
            .with_genesis()?
            .inspect(|this| {
                info!(target: "reth::cli", "\n{}", this.chain_spec().display_hardforks());
            });

        // setup the consensus instance
        let consensus: Arc<dyn Consensus> = if ctx.is_dev() {
            Arc::new(AutoSealConsensus::new(ctx.chain_spec()))
        } else {
            Arc::new(BeaconConsensus::new(ctx.chain_spec()))
        };

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        ctx.task_executor().spawn_critical("stages metrics listener task", sync_metrics_listener);

        // fetch the head block from the database
        let head = ctx.lookup_head()?;

        // Configure the blockchain tree for the node
        let tree_config = BlockchainTreeConfig::default();

        // NOTE: This is a temporary workaround to provide the canon state notification sender to the components builder because there's a cyclic dependency between the blockchain provider and the tree component. This will be removed once the Blockchain provider no longer depends on an instance of the tree: <https://github.com/paradigmxyz/reth/issues/7154>
        let (canon_state_notification_sender, _receiver) =
            tokio::sync::broadcast::channel(tree_config.max_reorg_depth() as usize * 2);

        let blockchain_db = BlockchainProvider::new(
            ctx.provider_factory().clone(),
            Arc::new(NoopBlockchainTree::with_canon_state_notifications(
                canon_state_notification_sender.clone(),
            )),
        )?;

        let builder_ctx = BuilderContext::new(
            head,
            blockchain_db.clone(),
            ctx.task_executor().clone(),
            ctx.data_dir().clone(),
            ctx.node_config().clone(),
            ctx.toml_config().clone(),
        );

        debug!(target: "reth::cli", "creating components");
        let components = match components_builder.build_components(&builder_ctx, None, None).await {
            Ok((components, _)) => components,
            Err(err) => {
                error!(target: "reth::cli", "Failed to build components: {}", err);
                return Err(err);
            }
        };

        let tree_externals = TreeExternals::new(
            ctx.provider_factory().clone(),
            consensus.clone(),
            EvmProcessorFactory::new(ctx.chain_spec(), components.evm_config().clone()),
        );
        let tree = BlockchainTree::new(tree_externals, tree_config, ctx.prune_modes())?
            .with_sync_metrics_tx(sync_metrics_tx.clone())
            // Note: This is required because we need to ensure that both the components and the
            // tree are using the same channel for canon state notifications. This will be removed
            // once the Blockchain provider no longer depends on an instance of the tree
            .with_canon_state_notification_sender(canon_state_notification_sender);

        let canon_state_notification_sender = tree.canon_state_notification_sender();
        let blockchain_tree = Arc::new(ShareableBlockchainTree::new(tree));

        // Replace the tree component with the actual tree
        let blockchain_db = blockchain_db.with_tree(blockchain_tree);

        debug!(target: "reth::cli", "configured blockchain tree");

        let NodeHooks { on_component_initialized, on_node_started, .. } = hooks;

        let node_adapter = NodeAdapter {
            components,
            task_executor: ctx.task_executor().clone(),
            provider: blockchain_db.clone(),
        };

        debug!(target: "reth::cli", "calling on_component_initialized hook");
        on_component_initialized.on_event(node_adapter.clone())?;

        // spawn exexs
        let mut exex_handles = Vec::with_capacity(installed_exex.len());
        let mut exexs = Vec::with_capacity(installed_exex.len());
        for (id, exex) in installed_exex {
            // create a new exex handle
            let (handle, events, notifications) = ExExHandle::new(id.clone());
            exex_handles.push(handle);

            // create the launch context for the exex
            let context = ExExContext {
                head,
                provider: blockchain_db.clone(),
                task_executor: ctx.task_executor().clone(),
                data_dir: ctx.data_dir().clone(),
                config: ctx.node_config().clone(),
                reth_config: ctx.toml_config().clone(),
                pool: node_adapter.components.pool().clone(),
                events,
                notifications,
            };

            let executor = ctx.task_executor().clone();
            exexs.push(async move {
                debug!(target: "reth::cli", id, "spawning exex");
                let span = reth_tracing::tracing::info_span!("exex", id);
                let _enter = span.enter();

                // init the exex
                let exex = exex.launch(context).await.unwrap();

                // spawn it as a crit task
                executor.spawn_critical("exex", async move {
                    info!(target: "reth::cli", "ExEx started");
                    match exex.await {
                        Ok(_) => panic!("ExEx {id} finished. ExEx's should run indefinitely"),
                        Err(err) => panic!("ExEx {id} crashed: {err}"),
                    }
                });
            });
        }

        future::join_all(exexs).await;

        // spawn exex manager
        let exex_manager_handle = if !exex_handles.is_empty() {
            debug!(target: "reth::cli", "spawning exex manager");
            // todo(onbjerg): rm magic number
            let exex_manager = ExExManager::new(exex_handles, 1024);
            let exex_manager_handle = exex_manager.handle();
            ctx.task_executor().spawn_critical("exex manager", async move {
                exex_manager.await.expect("exex manager crashed");
            });

            // send notifications from the blockchain tree to exex manager
            let mut canon_state_notifications = blockchain_db.subscribe_to_canonical_state();
            let mut handle = exex_manager_handle.clone();
            ctx.task_executor().spawn_critical(
                "exex manager blockchain tree notifications",
                async move {
                    while let Ok(notification) = canon_state_notifications.recv().await {
                        handle.send_async(notification.into()).await.expect(
                            "blockchain tree notification could not be sent to exex manager",
                        );
                    }
                },
            );

            info!(target: "reth::cli", "ExEx Manager started");

            Some(exex_manager_handle)
        } else {
            None
        };

        // create pipeline
        let network_client = node_adapter.network().fetch_client().await?;
        let (consensus_engine_tx, mut consensus_engine_rx) = unbounded_channel();

        if let Some(skip_fcu_threshold) = ctx.node_config().debug.skip_fcu {
            debug!(target: "reth::cli", "spawning skip FCU task");
            let (skip_fcu_tx, skip_fcu_rx) = unbounded_channel();
            let engine_skip_fcu = EngineApiSkipFcu::new(skip_fcu_threshold);
            ctx.task_executor().spawn_critical(
                "skip FCU interceptor",
                engine_skip_fcu.intercept(consensus_engine_rx, skip_fcu_tx),
            );
            consensus_engine_rx = skip_fcu_rx;
        }

        if let Some(store_path) = ctx.node_config().debug.engine_api_store.clone() {
            debug!(target: "reth::cli", "spawning engine API store");
            let (engine_intercept_tx, engine_intercept_rx) = unbounded_channel();
            let engine_api_store = EngineApiStore::new(store_path);
            ctx.task_executor().spawn_critical(
                "engine api interceptor",
                engine_api_store.intercept(consensus_engine_rx, engine_intercept_tx),
            );
            consensus_engine_rx = engine_intercept_rx;
        };

        let max_block = ctx.max_block(network_client.clone()).await?;
        let mut hooks = EngineHooks::new();

        let static_file_producer = ctx.static_file_producer();
        let static_file_producer_events = static_file_producer.lock().events();
        hooks.add(StaticFileHook::new(
            static_file_producer.clone(),
            Box::new(ctx.task_executor().clone()),
        ));
        info!(target: "reth::cli", "StaticFileProducer initialized");

        // Configure the pipeline
        let pipeline_exex_handle =
            exex_manager_handle.clone().unwrap_or_else(ExExManagerHandle::empty);
        let (mut pipeline, client) = if ctx.is_dev() {
            info!(target: "reth::cli", "Starting Reth in dev mode");

            for (idx, (address, alloc)) in ctx.chain_spec().genesis.alloc.iter().enumerate() {
                info!(target: "reth::cli", "Allocated Genesis Account: {:02}. {} ({} ETH)", idx,
address.to_string(), format_ether(alloc.balance));
            }

            // install auto-seal
            let mining_mode =
                ctx.dev_mining_mode(node_adapter.components.pool().pending_transactions_listener());
            info!(target: "reth::cli", mode=%mining_mode, "configuring dev mining mode");

            let (_, client, mut task) = reth_auto_seal_consensus::AutoSealBuilder::new(
                ctx.chain_spec(),
                blockchain_db.clone(),
                node_adapter.components.pool().clone(),
                consensus_engine_tx.clone(),
                canon_state_notification_sender,
                mining_mode,
                node_adapter.components.evm_config().clone(),
            )
            .build();

            let mut pipeline = crate::setup::build_networked_pipeline(
                ctx.node_config(),
                &ctx.toml_config().stages,
                client.clone(),
                Arc::clone(&consensus),
                ctx.provider_factory().clone(),
                ctx.task_executor(),
                sync_metrics_tx,
                ctx.prune_config(),
                max_block,
                static_file_producer,
                node_adapter.components.evm_config().clone(),
                pipeline_exex_handle,
            )
            .await?;

            let pipeline_events = pipeline.events();
            task.set_pipeline_events(pipeline_events);
            debug!(target: "reth::cli", "Spawning auto mine task");
            ctx.task_executor().spawn(Box::pin(task));

            (pipeline, EitherDownloader::Left(client))
        } else {
            let pipeline = crate::setup::build_networked_pipeline(
                ctx.node_config(),
                &ctx.toml_config().stages,
                network_client.clone(),
                Arc::clone(&consensus),
                ctx.provider_factory().clone(),
                ctx.task_executor(),
                sync_metrics_tx,
                ctx.prune_config(),
                max_block,
                static_file_producer,
                node_adapter.components.evm_config().clone(),
                pipeline_exex_handle,
            )
            .await?;

            (pipeline, EitherDownloader::Right(network_client.clone()))
        };

        let pipeline_events = pipeline.events();

        let initial_target = ctx.initial_pipeline_target();

        let mut pruner_builder =
            ctx.pruner_builder().max_reorg_depth(tree_config.max_reorg_depth() as usize);
        if let Some(exex_manager_handle) = &exex_manager_handle {
            pruner_builder =
                pruner_builder.finished_exex_height(exex_manager_handle.finished_height());
        }

        let mut pruner = pruner_builder.build(ctx.provider_factory().clone());

        let pruner_events = pruner.events();
        info!(target: "reth::cli", prune_config=?ctx.prune_config().unwrap_or_default(), "Pruner initialized");
        hooks.add(PruneHook::new(pruner, Box::new(ctx.task_executor().clone())));

        // Configure the consensus engine
        let (beacon_consensus_engine, beacon_engine_handle) = BeaconConsensusEngine::with_channel(
            client,
            pipeline,
            blockchain_db.clone(),
            Box::new(ctx.task_executor().clone()),
            Box::new(node_adapter.components.network().clone()),
            max_block,
            ctx.node_config().debug.continuous,
            node_adapter.components.payload_builder().clone(),
            initial_target,
            reth_beacon_consensus::MIN_BLOCKS_FOR_PIPELINE_RUN,
            consensus_engine_tx,
            consensus_engine_rx,
            hooks,
        )?;
        info!(target: "reth::cli", "Consensus engine initialized");

        let events = stream_select!(
            node_adapter.components.network().event_listener().map(Into::into),
            beacon_engine_handle.event_listener().map(Into::into),
            pipeline_events.map(Into::into),
            if ctx.node_config().debug.tip.is_none() && !ctx.is_dev() {
                Either::Left(
                    ConsensusLayerHealthEvents::new(Box::new(blockchain_db.clone()))
                        .map(Into::into),
                )
            } else {
                Either::Right(stream::empty())
            },
            pruner_events.map(Into::into),
            static_file_producer_events.map(Into::into)
        );
        ctx.task_executor().spawn_critical(
            "events task",
            node::handle_events(
                Some(node_adapter.components.network().clone()),
                Some(head.number),
                events,
                database.clone(),
            ),
        );

        let engine_api = EngineApi::new(
            blockchain_db.clone(),
            ctx.chain_spec(),
            beacon_engine_handle,
            node_adapter.components.payload_builder().clone().into(),
            Box::new(ctx.task_executor().clone()),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // extract the jwt secret from the args if possible
        let jwt_secret = ctx.auth_jwt_secret()?;

        // Start RPC servers
        let (rpc_server_handles, mut rpc_registry) = crate::rpc::launch_rpc_servers(
            node_adapter.clone(),
            engine_api,
            ctx.node_config(),
            jwt_secret,
            rpc,
        )
        .await?;

        // in dev mode we generate 20 random dev-signer accounts
        if ctx.is_dev() {
            rpc_registry.eth_api().with_dev_accounts();
        }

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        ctx.task_executor().spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

        let full_node = FullNode {
            evm_config: node_adapter.components.evm_config().clone(),
            pool: node_adapter.components.pool().clone(),
            network: node_adapter.components.network().clone(),
            provider: node_adapter.provider.clone(),
            payload_builder: node_adapter.components.payload_builder().clone(),
            task_executor: ctx.task_executor().clone(),
            rpc_server_handles,
            rpc_registry,
            config: ctx.node_config().clone(),
            data_dir: ctx.data_dir().clone(),
        };
        // Notify on node started
        on_node_started.on_event(full_node.clone())?;

        let handle = NodeHandle {
            node_exit_future: NodeExitFuture::new(rx, full_node.config.debug.terminate),
            node: full_node,
        };

        Ok(handle)
    }

    /// Launches the PoA node, also adding any RPC extensions passed.
    async fn launch_poa_node(
        self,
        target: NodeBuilderWithComponents<T, CB>,
    ) -> eyre::Result<Self::Node> {
        let Self { ctx } = self;
        let NodeBuilderWithComponents {
            adapter: NodeTypesAdapter { database },
            components_builder,
            add_ons: NodeAddOns { hooks, rpc, exexs: installed_exex },
            config,
        } = target;

        let btc_server = config.rpc.btc_server.clone();
        let bitcoind = config.rpc.bitcoind.clone();
        let chain = config.chain.clone();
        let frost = config.rpc.frost.clone();
        let btc_network = config.rpc.btc_network.clone();

        // setup the launch context
        let ctx = ctx
            .with_configured_globals()
            // load the toml config
            .with_loaded_toml_config(config)?
            // attach the database
            .attach(database.clone())
            // ensure certain settings take effect
            .with_adjusted_configs()
            // Create the provider factory
            .with_provider_factory()?
            .inspect(|_| {
                info!(target: "reth::cli", "Database opened");
            })
            .with_prometheus().await?
            .inspect(|this| {
                debug!(target: "reth::cli", chain=%this.chain_id(), genesis=?this.genesis_hash(), "Initializing genesis");
            })
            .with_genesis()?
            .inspect(|this| {
                info!(target: "reth::cli", "\n{}", this.chain_spec().display_hardforks());
            });

        // async task that checks system clock is in sync with NTP server
        ctx.task_executor().spawn_critical(
            "async system clock sync with ntp task",
            Box::pin(async {
                let sleep_sec = Duration::from_secs(15);
                let acceptable_drift_sec = 1;
                loop {
                    // TODO (scott) pass in ntp url as arg
                    match poa::ntp_unix_timestamp("time.cloudflare.com").await {
                        Ok(ntp_timestamp) => {
                            let system_timestamp = unix_timestamp();
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
        let jwt_secret = ctx.auth_jwt_secret()?;

        // Connect to btc signining server
        let btc_server_client = BtcServerExtendedClient::new(btc_server, Some(jwt_secret.clone()))
            .await
            .expect("cannot create btc_server");
        info!(target: "reth::cli", "Btc server connected");

        let bitcoin_block_headers: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>> =
            Arc::new(RwLock::new(None));
        let bitcoin_block_headers_clone = bitcoin_block_headers.clone();

        // create bitcoind client and make sure its synced
        let bitcoind_config: BitcoindConfig = bitcoind.clone().into();
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
        let is_testnet = is_testnet(chain.chain.id().clone());
        let confirmation_depth = get_confirmation_depth(is_testnet);
        let bitcoin_block_tx_ids: Arc<RwLock<HashMap<u64, Vec<bitcoin::Txid>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let bitcoin_block_tx_ids_clone = bitcoin_block_tx_ids.clone();
        let bitcoind_config_clone = bitcoind_config.clone();

        ctx.task_executor().spawn_critical("async bitcoin block tx ids task", Box::pin(async move {
            let sleep_ms = Duration::from_millis(5000);
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
        ctx.task_executor().spawn_critical(
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

        // setup the consensus instance
        let consensus: Arc<dyn Consensus> = if ctx.is_dev() {
            Arc::new(AutoSealConsensus::new(ctx.chain_spec()))
        } else {
            Arc::new(BeaconConsensus::new(ctx.chain_spec()))
        };

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        ctx.task_executor().spawn_critical("stages metrics listener task", sync_metrics_listener);

        // fetch the head block from the database
        let head = ctx.lookup_head()?;

        // Configure the blockchain tree for the node
        let tree_config = BlockchainTreeConfig::default();

        // NOTE: This is a temporary workaround to provide the canon state notification sender to the components builder because there's a cyclic dependency between the blockchain provider and the tree component. This will be removed once the Blockchain provider no longer depends on an instance of the tree: <https://github.com/paradigmxyz/reth/issues/7154>
        let (canon_state_notification_sender, _receiver) =
            tokio::sync::broadcast::channel(tree_config.max_reorg_depth() as usize * 2);

        let blockchain_db = BlockchainProvider::new(
            ctx.provider_factory().clone(),
            Arc::new(NoopBlockchainTree::with_canon_state_notifications(
                canon_state_notification_sender.clone(),
            )),
        )?;

        // get node secret key
        let network_sk = get_secret_key(&ctx.data_dir().clone().p2p_secret_path())?;

        // create authority config
        let (authority_index, authorities) = get_authority_signer_index(
            blockchain_db.clone(),
            Arc::clone(&chain.clone()),
            secp256k1::Secp256k1::new(),
            network_sk.clone(),
        )
        .expect("Failed to get authority index");

        // create frost config
        let mut frost_config: FrostConfig = frost.into();
        frost_config.set_authority_index(authority_index);
        frost_config.set_authorities(authorities);

        // Set up block import structures
        let (block_import_tx, block_import_rx) = unbounded_channel();
        let block_import = ProofOfAuthorityBlockImport::new(chain.clone(), block_import_tx);

        let builder_ctx = BuilderContext::new(
            head,
            blockchain_db.clone(),
            ctx.task_executor().clone(),
            ctx.data_dir().clone(),
            ctx.node_config().clone(),
            ctx.toml_config().clone(),
        );

        debug!(target: "reth::cli", "creating components");
        // build network with PoA block import and Frost capabilities
        let components_with_frost_handle = match components_builder
            .build_components(
                &builder_ctx,
                Some(Box::new(block_import)),
                Some(frost_config.clone()),
            )
            .await
        {
            Ok(components_with_frost_handle) => components_with_frost_handle,
            Err(err) => {
                error!(target: "reth::cli", "Failed to build components: {}", err);
                return Err(err);
            }
        };
        let (components, frost_handle) = components_with_frost_handle;

        let (consensus_engine_tx, mut consensus_engine_rx) = unbounded_channel();
        let bitcoind_config: BitcoindConfig = bitcoind.clone().into();
        let (
            _,
            mut block_fetcher_task,
            mut frost_task,
            mut sync_controller,
        ) = AuthorityConsensusBuilder::try_new(
            Arc::clone(&chain.clone()),
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
            components.network().clone(),
            frost_handle.clone(),
            block_import_rx,
            ctx.task_executor().clone(),
            components.evm_config().clone(),
            frost_config,
            btc_network,
        )
        .expect("Failed to create authority consensus builder")
        .build();

        let tree_externals = TreeExternals::new(
            ctx.provider_factory().clone(),
            consensus.clone(),
            EvmProcessorFactory::new(ctx.chain_spec(), components.evm_config().clone()),
        );
        let tree = BlockchainTree::new(tree_externals, tree_config, ctx.prune_modes())?
            .with_sync_metrics_tx(sync_metrics_tx.clone())
            // Note: This is required because we need to ensure that both the components and the
            // tree are using the same channel for canon state notifications. This will be removed
            // once the Blockchain provider no longer depends on an instance of the tree
            .with_canon_state_notification_sender(canon_state_notification_sender);

        let canon_state_notification_sender = tree.canon_state_notification_sender();
        let blockchain_tree = Arc::new(ShareableBlockchainTree::new(tree));

        // Replace the tree component with the actual tree
        let blockchain_db = blockchain_db.with_tree(blockchain_tree);

        debug!(target: "reth::cli", "configured blockchain tree");

        let NodeHooks { on_component_initialized, on_node_started, .. } = hooks;

        let node_adapter = NodeAdapter {
            components,
            task_executor: ctx.task_executor().clone(),
            provider: blockchain_db.clone(),
        };

        debug!(target: "reth::cli", "calling on_component_initialized hook");
        on_component_initialized.on_event(node_adapter.clone())?;

        // spawn exexs
        let mut exex_handles = Vec::with_capacity(installed_exex.len());
        let mut exexs = Vec::with_capacity(installed_exex.len());
        for (id, exex) in installed_exex {
            // create a new exex handle
            let (handle, events, notifications) = ExExHandle::new(id.clone());
            exex_handles.push(handle);

            // create the launch context for the exex
            let context = ExExContext {
                head,
                provider: blockchain_db.clone(),
                task_executor: ctx.task_executor().clone(),
                data_dir: ctx.data_dir().clone(),
                config: ctx.node_config().clone(),
                reth_config: ctx.toml_config().clone(),
                pool: node_adapter.components.pool().clone(),
                events,
                notifications,
            };

            let executor = ctx.task_executor().clone();
            exexs.push(async move {
                debug!(target: "reth::cli", id, "spawning exex");
                let span = reth_tracing::tracing::info_span!("exex", id);
                let _enter = span.enter();

                // init the exex
                let exex = exex.launch(context).await.unwrap();

                // spawn it as a crit task
                executor.spawn_critical("exex", async move {
                    info!(target: "reth::cli", "ExEx started");
                    match exex.await {
                        Ok(_) => panic!("ExEx {id} finished. ExEx's should run indefinitely"),
                        Err(err) => panic!("ExEx {id} crashed: {err}"),
                    }
                });
            });
        }

        future::join_all(exexs).await;

        // spawn exex manager
        let exex_manager_handle = if !exex_handles.is_empty() {
            debug!(target: "reth::cli", "spawning exex manager");
            // todo(onbjerg): rm magic number
            let exex_manager = ExExManager::new(exex_handles, 1024);
            let exex_manager_handle = exex_manager.handle();
            ctx.task_executor().spawn_critical("exex manager", async move {
                exex_manager.await.expect("exex manager crashed");
            });

            // send notifications from the blockchain tree to exex manager
            let mut canon_state_notifications = blockchain_db.subscribe_to_canonical_state();
            let mut handle = exex_manager_handle.clone();
            ctx.task_executor().spawn_critical(
                "exex manager blockchain tree notifications",
                async move {
                    while let Ok(notification) = canon_state_notifications.recv().await {
                        handle.send_async(notification.into()).await.expect(
                            "blockchain tree notification could not be sent to exex manager",
                        );
                    }
                },
            );

            info!(target: "reth::cli", "ExEx Manager started");

            Some(exex_manager_handle)
        } else {
            None
        };

        // create pipeline
        let network_client = node_adapter.network().fetch_client().await?;
        // let (consensus_engine_tx, mut consensus_engine_rx) = unbounded_channel();

        if let Some(skip_fcu_threshold) = ctx.node_config().debug.skip_fcu {
            debug!(target: "reth::cli", "spawning skip FCU task");
            let (skip_fcu_tx, skip_fcu_rx) = unbounded_channel();
            let engine_skip_fcu = EngineApiSkipFcu::new(skip_fcu_threshold);
            ctx.task_executor().spawn_critical(
                "skip FCU interceptor",
                engine_skip_fcu.intercept(consensus_engine_rx, skip_fcu_tx),
            );
            consensus_engine_rx = skip_fcu_rx;
        }

        if let Some(store_path) = ctx.node_config().debug.engine_api_store.clone() {
            debug!(target: "reth::cli", "spawning engine API store");
            let (engine_intercept_tx, engine_intercept_rx) = unbounded_channel();
            let engine_api_store = EngineApiStore::new(store_path);
            ctx.task_executor().spawn_critical(
                "engine api interceptor",
                engine_api_store.intercept(consensus_engine_rx, engine_intercept_tx),
            );
            consensus_engine_rx = engine_intercept_rx;
        };

        let max_block = ctx.max_block(network_client.clone()).await?;
        let mut hooks = EngineHooks::new();

        let static_file_producer = ctx.static_file_producer();
        let static_file_producer_events = static_file_producer.lock().events();
        hooks.add(StaticFileHook::new(
            static_file_producer.clone(),
            Box::new(ctx.task_executor().clone()),
        ));
        info!(target: "reth::cli", "StaticFileProducer initialized");

        // Configure the pipeline
        let pipeline_exex_handle =
            exex_manager_handle.clone().unwrap_or_else(ExExManagerHandle::empty);
        let (mut pipeline, client) = if ctx.is_dev() {
            info!(target: "reth::cli", "Starting Reth in dev mode");

            for (idx, (address, alloc)) in ctx.chain_spec().genesis.alloc.iter().enumerate() {
                info!(target: "reth::cli", "Allocated Genesis Account: {:02}. {} ({} ETH)", idx,
address.to_string(), format_ether(alloc.balance));
            }

            // install auto-seal
            let mining_mode =
                ctx.dev_mining_mode(node_adapter.components.pool().pending_transactions_listener());
            info!(target: "reth::cli", mode=%mining_mode, "configuring dev mining mode");

            let (_, client, mut task) = reth_auto_seal_consensus::AutoSealBuilder::new(
                ctx.chain_spec(),
                blockchain_db.clone(),
                node_adapter.components.pool().clone(),
                consensus_engine_tx.clone(),
                canon_state_notification_sender,
                mining_mode,
                node_adapter.components.evm_config().clone(),
            )
            .build();

            let mut pipeline = crate::setup::build_networked_pipeline(
                ctx.node_config(),
                &ctx.toml_config().stages,
                client.clone(),
                Arc::clone(&consensus),
                ctx.provider_factory().clone(),
                ctx.task_executor(),
                sync_metrics_tx,
                ctx.prune_config(),
                max_block,
                static_file_producer,
                node_adapter.components.evm_config().clone(),
                pipeline_exex_handle,
            )
            .await?;

            let pipeline_events = pipeline.events();
            task.set_pipeline_events(pipeline_events);
            debug!(target: "reth::cli", "Spawning auto mine task");
            ctx.task_executor().spawn(Box::pin(task));

            (pipeline, EitherDownloader::Left(client))
        } else {
            let pipeline = crate::setup::build_networked_pipeline(
                ctx.node_config(),
                &ctx.toml_config().stages,
                network_client.clone(),
                Arc::clone(&consensus),
                ctx.provider_factory().clone(),
                ctx.task_executor(),
                sync_metrics_tx,
                ctx.prune_config(),
                max_block,
                static_file_producer,
                node_adapter.components.evm_config().clone(),
                pipeline_exex_handle,
            )
            .await?;

            (pipeline, EitherDownloader::Right(network_client.clone()))
        };

        let pipeline_events = pipeline.events();

        let initial_target = ctx.initial_pipeline_target();

        let mut pruner_builder =
            ctx.pruner_builder().max_reorg_depth(tree_config.max_reorg_depth() as usize);
        if let Some(exex_manager_handle) = &exex_manager_handle {
            pruner_builder =
                pruner_builder.finished_exex_height(exex_manager_handle.finished_height());
        }

        let mut pruner = pruner_builder.build(ctx.provider_factory().clone());

        let pruner_events = pruner.events();
        info!(target: "reth::cli", prune_config=?ctx.prune_config().unwrap_or_default(), "Pruner initialized");
        hooks.add(PruneHook::new(pruner, Box::new(ctx.task_executor().clone())));

        // Configure the consensus engine
        let (beacon_consensus_engine, beacon_engine_handle) = BeaconConsensusEngine::with_channel(
            client,
            pipeline,
            blockchain_db.clone(),
            Box::new(ctx.task_executor().clone()),
            Box::new(node_adapter.components.network().clone()),
            max_block,
            ctx.node_config().debug.continuous,
            node_adapter.components.payload_builder().clone(),
            initial_target,
            reth_beacon_consensus::MIN_BLOCKS_FOR_PIPELINE_RUN,
            consensus_engine_tx,
            consensus_engine_rx,
            hooks,
        )?;
        info!(target: "reth::cli", "Consensus engine initialized");

        let events = stream_select!(
            node_adapter.components.network().event_listener().map(Into::into),
            beacon_engine_handle.event_listener().map(Into::into),
            pipeline_events.map(Into::into),
            if ctx.node_config().debug.tip.is_none() && !ctx.is_dev() {
                Either::Left(
                    ConsensusLayerHealthEvents::new(Box::new(blockchain_db.clone()))
                        .map(Into::into),
                )
            } else {
                Either::Right(stream::empty())
            },
            pruner_events.map(Into::into),
            static_file_producer_events.map(Into::into)
        );
        ctx.task_executor().spawn_critical(
            "events task",
            node::handle_events(
                Some(node_adapter.components.network().clone()),
                Some(head.number),
                events,
                database.clone(),
            ),
        );

        let engine_api = EngineApi::new(
            blockchain_db.clone(),
            ctx.chain_spec(),
            beacon_engine_handle,
            node_adapter.components.payload_builder().clone().into(),
            Box::new(ctx.task_executor().clone()),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // Start RPC servers
        let (rpc_server_handles, mut rpc_registry) = crate::rpc::launch_rpc_servers(
            node_adapter.clone(),
            engine_api,
            ctx.node_config(),
            jwt_secret,
            rpc,
        )
        .await?;

        // in dev mode we generate 20 random dev-signer accounts
        if ctx.is_dev() {
            rpc_registry.eth_api().with_dev_accounts();
        }

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        ctx.task_executor().spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

        let full_node = FullNode {
            evm_config: node_adapter.components.evm_config().clone(),
            pool: node_adapter.components.pool().clone(),
            network: node_adapter.components.network().clone(),
            provider: node_adapter.provider.clone(),
            payload_builder: node_adapter.components.payload_builder().clone(),
            task_executor: ctx.task_executor().clone(),
            rpc_server_handles,
            rpc_registry,
            config: ctx.node_config().clone(),
            data_dir: ctx.data_dir().clone(),
        };
        // Notify on node started
        on_node_started.on_event(full_node.clone())?;

        let handle = NodeHandle {
            node_exit_future: NodeExitFuture::new(rx, full_node.config.debug.terminate),
            node: full_node,
        };

        Ok(handle)
    }
}
