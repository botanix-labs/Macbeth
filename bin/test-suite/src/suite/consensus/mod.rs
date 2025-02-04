use self::common::botanix_client::BotanixEthClient;
use super::{Outcome, Suite};
use crate::{
    context::GlobalContext,
    it_info_print, it_warn_print, run_test,
    suite::consensus::common::{
        btc_server::{spawn_n_btc_server_processes, SpawnedBtcServerProcess},
        events::await_dkg,
    },
};
use anyhow::Context;
use async_trait::async_trait;
use client::BtcServerClient;
use comet_bft_rpc::{CometBftRpcFactory, HttpCometBFTRpcClientFactory};
use common::{
    bitcoind_node::{
        create_bitcoind_node, BitcoindNodeConfig, Notifications as BitcoindNotifications,
        SpawnedBitcoindProcess,
    },
    comet_node::{
        create_cometbft_nodes, CometBftNodeConfig, Notifications as CometbftNotifications,
        SpawnedCometBftProcess,
    },
    create_botanix_eth_client, kill_process_at_port,
    poa_node::{
        create_poa_nodes, FederationMemberTestConfig, Notifications as PoaNodeNotifications,
        SpawnedPoaServerProcess,
    },
    rpc_node::{
        create_rpc_nodes, NonFederationMemberTestConfig, Notifications as RpcNotifications,
        SpawnedRpcServerProcess,
    },
};
use reth_btc_wallet::bitcoind::{BitcoindClientFactory, BitcoindConfig, BitcoindFactory};
use reth_db::DatabaseEnv;
use reth_provider::ProviderFactory;
use reth_tracing::tracing::error;
use std::{
    collections::{BTreeMap, HashSet},
    panic,
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::Duration,
};
use tonic::transport::Channel;
use tracing::info;
// scopes
mod common;
mod frost;
mod invalid_transactions;
mod rpc_node;
mod sync;

fn split_members_at<T: Clone>(
    map: BTreeMap<u16, T>,
    at: usize,
) -> (BTreeMap<u16, T>, BTreeMap<u16, T>) {
    let entries: Vec<_> = map.into_iter().collect();
    let (left, right) = entries.split_at(at);
    (BTreeMap::from_iter(left.iter().cloned()), BTreeMap::from_iter(right.iter().cloned()))
}

pub struct ConsensusIntegrationTestSuite {
    pub timeout: Duration,
    pub global_context: Arc<GlobalContext>,
    pub outcomes: Vec<Outcome>,
    pub local_context: LocalContext,
}

pub struct LocalContext {
    // bitcoind
    pub bitcoind_process: Option<SpawnedBitcoindProcess>,
    pub bitcoind_node: Option<BitcoindNodeConfig>,
    pub bitcoind_notification: Option<tokio::sync::broadcast::Sender<BitcoindNotifications>>,
    // btc
    pub btc_processes: Option<Vec<SpawnedBtcServerProcess>>,
    pub btc_server_clients: Option<Vec<BtcServerClient<Channel>>>,
    // poa
    pub poa_processes: Option<Vec<SpawnedPoaServerProcess>>,
    pub poa_nodes: Option<BTreeMap<u16, FederationMemberTestConfig>>,
    pub poa_notification: Option<tokio::sync::broadcast::Sender<PoaNodeNotifications>>,
    pub poa_eth_providers: Option<Vec<BotanixEthClient>>,
    // cometbft
    pub cometbft_processes: Option<Vec<SpawnedCometBftProcess>>,
    pub cometbft_nodes: Option<BTreeMap<u16, CometBftNodeConfig>>,
    pub cometbft_nodes_syncing: Option<BTreeMap<u16, CometBftNodeConfig>>,
    pub cometbft_notification: Option<tokio::sync::broadcast::Sender<CometbftNotifications>>,
    pub cometbft_lightclients: Option<Vec<HttpCometBFTRpcClientFactory>>,
    // rpc
    pub rpc_processes: Option<Vec<SpawnedRpcServerProcess>>,
    pub rpc_nodes: Option<BTreeMap<u16, NonFederationMemberTestConfig>>,
    pub rpc_notification: Option<tokio::sync::broadcast::Sender<RpcNotifications>>,
    pub rpc_eth_providers: Option<Vec<BotanixEthClient>>,
    // authority members in the federation
    pub authorities: Vec<secp256k1::PublicKey>,
}

impl LocalContext {
    // btc servers
    pub fn get_btc_server_process_port(&self, instance: usize) -> Option<u16> {
        self.btc_processes
            .as_ref()
            .and_then(|processes| processes.iter().nth(instance).map(|val| val.port))
    }

    pub fn get_btc_server_processes_ids(&self) -> Vec<u32> {
        self.btc_processes
            .as_ref()
            .map(|btc_processes| {
                btc_processes
                    .iter()
                    .map(|process| process.child_process.id())
                    .collect::<Vec<Option<u32>>>()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|process_id| process_id)
            .collect::<Vec<u32>>()
    }

    pub fn get_btc_server_processes_used_ports(&self) -> Vec<u16> {
        let ports = self
            .btc_processes
            .as_ref()
            .map(|btc_processes| {
                btc_processes.iter().map(|process| process.port).collect::<Vec<u16>>()
            })
            .unwrap_or_default();

        let hs: HashSet<u16> = HashSet::from_iter(ports);
        hs.into_iter().collect()
    }

    pub fn get_btc_processes_dbs(&self) -> Vec<PathBuf> {
        self.btc_processes
            .as_ref()
            .map(|btc_processes| {
                btc_processes
                    .iter()
                    .map(|process| process.db_path.clone())
                    .collect::<Vec<PathBuf>>()
            })
            .unwrap_or_default()
    }

    // poa nodes
    pub fn get_poa_processes_ids(&self) -> Vec<u32> {
        self.poa_processes
            .as_ref()
            .map(|poa_processes| {
                poa_processes
                    .iter()
                    .map(|process| process.child_process.id())
                    .collect::<Vec<Option<u32>>>()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|process_id| process_id)
            .collect::<Vec<u32>>()
    }

    pub fn get_poa_processes_rpc_ports(&self) -> Vec<u16> {
        let rpc_ports = self
            .poa_processes
            .as_ref()
            .map(|poa_processes| {
                poa_processes.iter().map(|process| process.rpc_port).collect::<Vec<u16>>()
            })
            .unwrap_or_default();

        let hs: HashSet<u16> = HashSet::from_iter(rpc_ports);
        hs.into_iter().collect()
    }

    pub fn get_poa_processes_discovery_ports(&self) -> Vec<u16> {
        let disovery_ports = self
            .poa_processes
            .as_ref()
            .map(|poa_processes| {
                poa_processes.iter().map(|process| process.discovery_port).collect::<Vec<u16>>()
            })
            .unwrap_or_default();

        let hs: HashSet<u16> = HashSet::from_iter(disovery_ports);
        hs.into_iter().collect()
    }

    // rpc nodes
    pub fn get_rpc_processes_ids(&self) -> Vec<u32> {
        self.rpc_processes
            .as_ref()
            .map(|rpc_processes| {
                rpc_processes
                    .iter()
                    .map(|process| process.child_process.id())
                    .collect::<Vec<Option<u32>>>()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|process_id| process_id)
            .collect::<Vec<u32>>()
    }

    pub fn get_rpc_processes_rpc_ports(&self) -> Vec<u16> {
        let rpc_ports = self
            .rpc_processes
            .as_ref()
            .map(|rpc_processes| {
                rpc_processes.iter().map(|process| process.rpc_port).collect::<Vec<u16>>()
            })
            .unwrap_or_default();

        let hs: HashSet<u16> = HashSet::from_iter(rpc_ports);
        hs.into_iter().collect()
    }

    pub fn get_dbs(&self) -> Vec<ProviderFactory<Arc<DatabaseEnv>>> {
        let db_provider_factories = self
            .poa_processes
            .as_ref()
            .map(|poa_processes| {
                poa_processes
                    .iter()
                    .map(|process| process.provider_factory.clone())
                    .collect::<Vec<ProviderFactory<Arc<DatabaseEnv>>>>()
            })
            .unwrap_or_default();
        db_provider_factories
    }

    pub fn get_rpc_processes_discovery_ports(&self) -> Vec<u16> {
        let disovery_ports = self
            .rpc_processes
            .as_ref()
            .map(|rpc_processes| {
                rpc_processes.iter().map(|process| process.discovery_port).collect::<Vec<u16>>()
            })
            .unwrap_or_default();

        let hs: HashSet<u16> = HashSet::from_iter(disovery_ports);
        hs.into_iter().collect()
    }

    // cometbft nodes
    pub fn get_cometbft_processes_ids(&self) -> Vec<u32> {
        self.cometbft_processes
            .as_ref()
            .map(|cometbft_processes| {
                cometbft_processes
                    .iter()
                    .map(|process| process.child_process.id())
                    .collect::<Vec<Option<u32>>>()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|process_id| process_id)
            .collect::<Vec<u32>>()
    }

    pub fn get_cometbft_processes_rpc_ports(&self) -> Vec<u16> {
        let rpc_ports = self
            .cometbft_processes
            .as_ref()
            .map(|cometbft_processes| {
                cometbft_processes
                    .iter()
                    .map(|process| process.cometbft_rpc_app_port)
                    .collect::<Vec<u16>>()
            })
            .unwrap_or_default();

        let hs: HashSet<u16> = HashSet::from_iter(rpc_ports);
        hs.into_iter().collect()
    }

    pub fn get_cometbft_processes_proxy_ports(&self) -> Vec<u16> {
        let proxy_ports = self
            .cometbft_processes
            .as_ref()
            .map(|cometbft_processes| {
                cometbft_processes
                    .iter()
                    .map(|process| process.cometbft_proxy_app_port)
                    .collect::<Vec<u16>>()
            })
            .unwrap_or_default();

        let hs: HashSet<u16> = HashSet::from_iter(proxy_ports);
        hs.into_iter().collect()
    }

    pub fn get_cometbft_processes_p2p_ports(&self) -> Vec<u16> {
        let p2p_ports = self
            .cometbft_processes
            .as_ref()
            .map(|cometbft_processes| {
                cometbft_processes
                    .iter()
                    .map(|process| process.cometbft_p2p_app_port)
                    .collect::<Vec<u16>>()
            })
            .unwrap_or_default();

        let hs: HashSet<u16> = HashSet::from_iter(p2p_ports);
        hs.into_iter().collect()
    }

    // bitcoind node
    pub fn get_bitcoind_process_id(&self) -> u32 {
        self.bitcoind_process
            .as_ref()
            .map(|bitcoind_process| bitcoind_process.child_process.id())
            .flatten()
            .unwrap_or_default()
    }

    pub fn get_bitcoind_process_port(&self) -> u16 {
        self.bitcoind_process
            .as_ref()
            .map(|bitcoind_process| bitcoind_process.port)
            .unwrap_or_default()
    }
}

pub struct CreateTestConfig {
    pub create_bitcoind_node: bool,
    pub create_poa_nodes: bool,
    pub create_rpc_nodes: bool,
    pub create_btc_servers: bool,
    pub create_cometbft_nodes: bool,
    pub create_state_syncing_node: bool,
}

impl CreateTestConfig {
    #[allow(dead_code)]
    fn full_scope() -> Self {
        Self {
            create_bitcoind_node: true,
            create_poa_nodes: true,
            create_rpc_nodes: true,
            create_btc_servers: true,
            create_cometbft_nodes: true,
            create_state_syncing_node: true,
        }
    }
}

impl Default for CreateTestConfig {
    fn default() -> Self {
        Self {
            create_bitcoind_node: false,
            create_poa_nodes: false,
            create_rpc_nodes: false,
            create_btc_servers: false,
            create_cometbft_nodes: false,
            create_state_syncing_node: false,
        }
    }
}

#[async_trait]
impl Suite for ConsensusIntegrationTestSuite {
    fn name(&self) -> &str {
        "ConsensusIntegrationTestSuite"
    }

    async fn run(&mut self, test_to_run: String) -> Vec<Outcome> {
        self.set_panic_hook();
        match test_to_run.as_str() {
            "dkg_flow" => run_test!(
                self,
                CreateTestConfig {
                    create_bitcoind_node: true,
                    create_btc_servers: true,
                    ..Default::default()
                },
                frost::test_dkg::dkg_flow
            ),
            "signing_flow" => run_test!(
                self,
                CreateTestConfig {
                    create_bitcoind_node: true,
                    create_btc_servers: true,
                    ..Default::default()
                },
                frost::test_signing::test_many_inputs_signing
            ),
            "utxo_commitment" => run_test!(
                self,
                CreateTestConfig {
                    create_bitcoind_node: true,
                    create_btc_servers: true,
                    ..Default::default()
                },
                frost::test_utxo_commitment::test_utxo_commitment
            ),
            "block_builder" => {
                run_test!(
                    self,
                    CreateTestConfig {
                        create_bitcoind_node: true,
                        create_poa_nodes: true,
                        create_btc_servers: true,
                        create_cometbft_nodes: true,
                        ..Default::default()
                    },
                    frost::test_block_builder::block_builder
                )
            }
            "batch_pegins" => {
                run_test!(
                    self,
                    CreateTestConfig {
                        create_bitcoind_node: true,
                        create_poa_nodes: true,
                        create_btc_servers: true,
                        create_cometbft_nodes: true,
                        ..Default::default()
                    },
                    frost::test_batch_pegins::batch_pegins
                )
            }
            "utxo_sync" => {
                run_test!(
                    self,
                    CreateTestConfig {
                        create_bitcoind_node: true,
                        create_poa_nodes: true,
                        create_btc_servers: true,
                        create_cometbft_nodes: true,
                        ..Default::default()
                    },
                    frost::test_utxo_sync::utxo_sync
                )
            }
            "frost_e2e_stable" => {
                run_test!(
                    self,
                    CreateTestConfig {
                        create_bitcoind_node: true,
                        create_poa_nodes: true,
                        create_btc_servers: true,
                        create_cometbft_nodes: true,
                        ..Default::default()
                    },
                    frost::test_frost_e2e::frost_e2e_stable
                )
            }
            "state_sync" => {
                run_test!(
                    self,
                    CreateTestConfig {
                        create_bitcoind_node: true,
                        create_poa_nodes: true,
                        create_btc_servers: true,
                        create_cometbft_nodes: true,
                        ..Default::default()
                    },
                    sync::test_state_sync::test_state_sync
                )
            }
            "frost_e2e_failed_signing_disconnect" => {
                run_test!(
                    self,
                    CreateTestConfig {
                        create_bitcoind_node: true,
                        create_poa_nodes: true,
                        create_btc_servers: true,
                        create_cometbft_nodes: true,
                        ..Default::default()
                    },
                    frost::test_frost_e2e_signing_disconnect::frost_e2e_failed_signing_disconnect
                )
            }
            "e2e_peer_disconnect" => {
                run_test!(
                    self,
                    CreateTestConfig {
                        create_bitcoind_node: true,
                        create_poa_nodes: true,
                        create_btc_servers: true,
                        create_cometbft_nodes: true,
                        ..Default::default()
                    },
                    frost::test_e2e_peer_disconnect::e2e_peer_disconnect,
                )
            }
            "rpc_node" => {
                run_test!(
                    self,
                    CreateTestConfig {
                        create_bitcoind_node: true,
                        create_rpc_nodes: true,
                        create_poa_nodes: true,
                        create_btc_servers: true,
                        create_cometbft_nodes: true,
                        ..Default::default()
                    },
                    rpc_node::test_rpc_node::test_rpc_node
                )
            }
            "invalid_pegin" => {
                run_test!(
                    self,
                    CreateTestConfig {
                        create_bitcoind_node: true,
                        create_poa_nodes: true,
                        create_btc_servers: true,
                        create_cometbft_nodes: true,
                        ..Default::default()
                    },
                    invalid_transactions::test_invalid_pegin::invalid_pegin
                )
            }
            "invalid_pegout" => {
                run_test!(
                    self,
                    CreateTestConfig {
                        create_bitcoind_node: true,
                        create_poa_nodes: true,
                        create_btc_servers: true,
                        create_cometbft_nodes: true,
                        ..Default::default()
                    },
                    invalid_transactions::test_invalid_pegout::invalid_pegout
                )
            }
            "test_mempool_gossip" => {
                run_test!(
                    self,
                    CreateTestConfig {
                        create_bitcoind_node: true,
                        create_poa_nodes: true,
                        create_btc_servers: true,
                        create_cometbft_nodes: true,
                        ..Default::default()
                    },
                    frost::test_mempool_gossip::test_mempool_gossip
                )
            }
            "test_conflicting_input" => run_test!(
                self,
                CreateTestConfig {
                    create_bitcoind_node: true,
                    create_btc_servers: true,
                    ..Default::default()
                },
                frost::test_conflicting_input::test_conflicting_input
            ),
            "test_round1_then_new_signing_session" => run_test!(
                self,
                CreateTestConfig {
                    create_bitcoind_node: true,
                    create_btc_servers: true,
                    ..Default::default()
                },
                frost::test_round1_then_new_signing_session::test_round1_then_new_signing_session
            ),
            _ => {
                error!("Test {:?} not found", test_to_run.as_str());
                return vec![];
            }
        };

        self.outcomes.clone()
    }

    fn set_panic_hook(&mut self) {
        // =================== BTC SERVERS ================== //
        let btc_server_processes_ids = self.local_context.get_btc_server_processes_ids();
        let btc_server_dbs = self.local_context.get_btc_processes_dbs();
        let btc_server_processes_used_ports =
            self.local_context.get_btc_server_processes_used_ports();

        // =================== POA NODES ================== //
        let poa_processes_ids = self.local_context.get_poa_processes_ids();
        let poa_processes_discovery_ports = self.local_context.get_poa_processes_discovery_ports();
        let poa_processes_rpc_ports = self.local_context.get_poa_processes_rpc_ports();

        // =================== PRC NODES ================== //
        let rpc_processes_ids = self.local_context.get_rpc_processes_ids();
        let rpc_processes_discovery_ports = self.local_context.get_rpc_processes_discovery_ports();
        let rpc_processes_rpc_ports = self.local_context.get_rpc_processes_rpc_ports();

        // =================== COMETBFT NODES ================== //
        let cometbft_processes_ids = self.local_context.get_cometbft_processes_ids();
        let cometbft_processes_rpc_ports = self.local_context.get_cometbft_processes_rpc_ports();
        let cometbft_processes_proxy_ports =
            self.local_context.get_cometbft_processes_proxy_ports();
        let cometbft_processes_p2p_ports = self.local_context.get_cometbft_processes_p2p_ports();

        // =================== BITCOIND NODE ================== //
        let bitcoind_process_id = self.local_context.get_bitcoind_process_id();
        let bitcoind_port = self.local_context.get_bitcoind_process_port();

        // set the panic hook so it kills them whenever activated
        std::panic::set_hook(Box::new(move |panic_info| {
            error!("Test suite panicked {:?}", panic_info);

            // =================== PRC NODES ================== //
            for rpc_processes_id in &rpc_processes_ids {
                // Send a termination signal to the child process
                let _ = Command::new("kill")
                    .arg("-9") // Use SIGKILL for immediate termination
                    .arg(format!("{rpc_processes_id}"))
                    .output();
            }
            // kill process at port
            for rpc_processes_discovery_port in &rpc_processes_discovery_ports {
                kill_process_at_port(*rpc_processes_discovery_port);
            }
            for rpc_processes_rpc_port in &rpc_processes_rpc_ports {
                kill_process_at_port(*rpc_processes_rpc_port);
            }
            // =================== POA NODES ================== //
            for poa_processes_id in &poa_processes_ids {
                // Send a termination signal to the child process
                let _ = Command::new("kill")
                    .arg("-9") // Use SIGKILL for immediate termination
                    .arg(format!("{poa_processes_id}"))
                    .output();
            }
            // kill process at port
            for poa_processes_discovery_port in &poa_processes_discovery_ports {
                kill_process_at_port(*poa_processes_discovery_port);
            }
            for poa_processes_rpc_port in &poa_processes_rpc_ports {
                kill_process_at_port(*poa_processes_rpc_port);
            }
            // =================== BTC SERVERS ================== //
            // kill the actual processes
            for btc_server_process_id in &btc_server_processes_ids {
                // Send a termination signal to the child process
                let _ = Command::new("kill")
                    .arg("-9") // Use SIGKILL for immediate termination
                    .arg(format!("{btc_server_process_id}"))
                    .output();
            }
            // delete dbs
            for btc_server_db in &btc_server_dbs {
                let _ = std::fs::remove_dir_all(btc_server_db.clone());
            }
            // kill process at port
            for btc_server_processes_used_port in &btc_server_processes_used_ports {
                kill_process_at_port(*btc_server_processes_used_port);
            }
            // =================== COMETBFT NODES ================== //
            for cometbft_processes_id in &cometbft_processes_ids {
                // Send a termination signal to the child process
                let _ = Command::new("kill")
                    .arg("-9") // Use SIGKILL for immediate termination
                    .arg(format!("{cometbft_processes_id}"))
                    .output();
            }
            // kill process at port
            for cometbft_processes_proxy_port in &cometbft_processes_proxy_ports {
                kill_process_at_port(*cometbft_processes_proxy_port);
            }
            for cometbft_processes_rpc_port in &cometbft_processes_rpc_ports {
                kill_process_at_port(*cometbft_processes_rpc_port);
            }
            for cometbft_processes_p2p_port in &cometbft_processes_p2p_ports {
                kill_process_at_port(*cometbft_processes_p2p_port);
            }

            // =================== BITCOIND NODE ================== //
            // Send a termination signal to the child process
            let _ = Command::new("kill")
                .arg("-9") // Use SIGKILL for immediate termination
                .arg(format!("{bitcoind_process_id}"))
                .output();
            kill_process_at_port(bitcoind_port);

            std::process::exit(1);
        }));
    }

    async fn destroy_local_context(&mut self) {
        it_info_print!("Destroying test suite context ...");

        // =================== RPC NODES ================== //
        if let Some(rpc_processes) = self.local_context.rpc_processes.as_mut() {
            for rpc_process in rpc_processes.iter_mut() {
                rpc_process.destroy_all_async().await
            }
        }

        // =================== POA NODES ================== //
        if let Some(poa_processes) = self.local_context.poa_processes.as_mut() {
            for poa_process in poa_processes.iter_mut() {
                poa_process.destroy_all_async().await
            }
        }

        // =================== BTC NODES ================== //
        if let Some(btc_processes) = self.local_context.btc_processes.as_mut() {
            for btc_process in btc_processes.iter_mut() {
                btc_process.destroy_all_async().await
            }
        }

        // =================== COMETBFT NODES ================== //
        if let Some(cometbft_processes) = self.local_context.cometbft_processes.as_mut() {
            for cometbft_process in cometbft_processes.iter_mut() {
                cometbft_process.destroy_all_async().await
            }
        }

        // =================== BITCOIND NODE ================== //
        if let Some(bitcoind_process) = self.local_context.bitcoind_process.as_mut() {
            bitcoind_process.destroy_all_async().await
        }
    }

    async fn create_new_local_context(
        &mut self,
        create_test_config: CreateTestConfig,
    ) -> anyhow::Result<()> {
        // =================== BITCOIND NODE ================== //
        if create_test_config.create_bitcoind_node {
            let (bitcoind_node, tx) = create_bitcoind_node(self.global_context.clone()).await?;
            it_info_print!("Starting bitcoind node ...");
            // spawn bitcoind node as a process
            let spawned_bitcoind_process = bitcoind_node.spawn_service()?;
            tokio::time::sleep(Duration::from_secs(6)).await;

            // await initialization
            bitcoind_node.await_initialization().await?;

            let bitcoind_factory = BitcoindClientFactory::new(BitcoindConfig::new(
                self.global_context.bitcoind_url.clone(),
                self.global_context.bitcoind_user.clone(),
                self.global_context.bitcoind_pass.clone(),
            ));
            let _bitcoind_client = bitcoind_factory.build_and_connect()?;

            // update local context
            self.local_context.bitcoind_process = Some(spawned_bitcoind_process);
            self.local_context.bitcoind_node = Some(bitcoind_node);
            self.local_context.bitcoind_notification = Some(tx);
        }

        // =================== BTC SERVERS ================== //
        let mut btc_server_clients = vec![];
        if create_test_config.create_btc_servers {
            it_info_print!("Starting btc servers ...");
            self.local_context.btc_processes =
                Some(spawn_n_btc_server_processes(self.global_context.clone())?);
            // let btc servers come up
            tokio::time::sleep(Duration::from_secs(5)).await;
            // try to connect to each btc server before moving on
            let mut tries = 5;
            let mut successes = 0;
            loop {
                it_info_print!("Trying to connect to all btc servers");
                if successes == self.global_context.fed_instances {
                    break;
                }
                if tries == 0 {
                    panic!("Failed to connect to all btc servers");
                }
                successes = 0;
                for instance in 0..self.global_context.fed_instances {
                    let port = self
                        .local_context
                        .get_btc_server_process_port(instance as usize)
                        .context("could not find btc server at instance index")?;
                    match client::BtcServerClient::connect(format!("http://localhost:{}", port))
                        .await
                    {
                        Ok(_) => {
                            it_info_print!("Connected to btc server at port", port.to_string());
                            successes += 1;
                        }
                        Err(e) => {
                            it_warn_print!(
                                "Failed to connect to btc server at port",
                                port.to_string(),
                                e
                            );
                        }
                    }

                    tries -= 1;
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                }
            }
            it_info_print!("Connected to all btc servers!");
            // Save the btc server clients and spawned processes in local context
            for instance in 0..self.global_context.fed_instances {
                let port = self
                    .local_context
                    .get_btc_server_process_port(instance as usize)
                    .context("could not find btc server at instance index")?;
                let client = client::BtcServerClient::connect(format!("http://localhost:{}", port))
                    .await
                    .context("Unable to create and connect to a btc server client")?;
                btc_server_clients.push(client.clone());
            }
            self.local_context.btc_server_clients = Some(btc_server_clients.clone());

            // short delay to prevent btc_server hitting `Unable to get public key`
            // when starting poa nodes in tests
            tokio::time::sleep(Duration::from_secs(5)).await;
        }

        // =================== COMMETBFT NODES ================== //
        let mut cometbft_lightclients = vec![];
        let mut spawned_cometbft_processes = vec![];
        if create_test_config.create_cometbft_nodes {
            it_info_print!("Starting cometbft nodes ...");
            let (cometbft_nodes, tx) = create_cometbft_nodes(self.global_context.clone()).await?;
            let poa_instances =
                self.global_context.fed_instances - self.global_context.syncing_instances;
            let (cometbft_nodes, cometbft_nodes_syncing) =
                split_members_at(cometbft_nodes, poa_instances as usize);

            for (_, cometbft_node) in cometbft_nodes.iter() {
                // spawn cometbft node as a process
                spawned_cometbft_processes.push(cometbft_node.spawn_service()?);

                // create cometbft client
                let cometbft_client = HttpCometBFTRpcClientFactory::new(
                    "0.0.0.0".to_string(),
                    cometbft_node.cometbft_rpc_app_port,
                );
                let _cometbft_http_client = cometbft_client.build_and_connect()?;
                cometbft_lightclients.push(cometbft_client);

                // await initialization
                cometbft_node.await_initialization()?;

                // wait for 5 seconds in between processes start
                tokio::time::sleep(Duration::from_secs(5)).await;
            }

            // update local context
            self.local_context.cometbft_processes = Some(spawned_cometbft_processes);
            self.local_context.cometbft_nodes = Some(cometbft_nodes);
            self.local_context.cometbft_nodes_syncing = Some(cometbft_nodes_syncing);
            self.local_context.cometbft_notification = Some(tx);
            self.local_context.cometbft_lightclients = Some(cometbft_lightclients);
        }

        // =================== POA NODES ================== //
        let mut poa_botanix_clients = vec![];
        if create_test_config.create_poa_nodes {
            // then generate test fed members poa nodes
            let (mut poa_nodes, tx, edh_authorities_list) = create_poa_nodes(
                self.global_context.clone(),
                self.local_context.btc_processes.as_ref(),
            )
            .await?;

            self.local_context.authorities = edh_authorities_list.clone();
            let build_command_authorities_list = Arc::new(edh_authorities_list);

            let mut spawned_poa_processes = vec![];

            it_info_print!("Starting poa nodes");
            let mut rx = tx.subscribe();
            for (index, poa_node) in poa_nodes.iter_mut() {
                it_info_print!("Starting poa node", index);
                let build_command_authorities_list = Arc::clone(&build_command_authorities_list);

                // spawn poa node as a process
                spawned_poa_processes.push(poa_node.spawn_service(build_command_authorities_list)?);

                // wait for two seconds in between processes start
                tokio::time::sleep(Duration::from_secs(5)).await;
            }

            // loop over the poa nodes and wait until they become initialized so the eth clients can
            // connect with them
            for (index, poa_node) in poa_nodes.iter_mut() {
                // create botanix client and await initialization
                let botanix_eth_client = loop {
                    match create_botanix_eth_client(poa_node.rpc_port, poa_node.ws_port).await {
                        Ok(client) => {
                            it_info_print!(
                                "Botanix client for poa member {} just connected!",
                                index
                            );
                            break client;
                        }
                        Err(_) => {
                            it_warn_print!("Btc-server {:?} not ready yet... Re-trying", index);
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                };
                poa_node.botanix_eth_client = Some(botanix_eth_client.clone());
                poa_botanix_clients.push(botanix_eth_client);
                it_info_print!("Botanix client created for poa member {}", index);

                // await initialization
                poa_node.await_initialization()?;
            }

            // run the dkg
            await_dkg(&mut poa_nodes, &mut rx).await;

            // At this point all the btc servers should have the same aggregate key
            let (btc_server_clients, _btc_server_clients_syncing) =
                btc_server_clients.split_at(self.global_context.fed_instances as usize);
            let mut keys = HashSet::new();
            for client in btc_server_clients.to_vec().iter_mut() {
                let key = client
                    .get_public_key(client::Empty {})
                    .await
                    .context("Error getting a pub key from btc-server")?
                    .into_inner()
                    .publickey;
                keys.insert(key);
            }
            // All keys should be the same
            assert_eq!(keys.len(), 1);

            // update local context
            self.local_context.poa_processes = Some(spawned_poa_processes);
            self.local_context.poa_nodes = Some(poa_nodes);
            self.local_context.poa_notification = Some(tx);
            self.local_context.poa_eth_providers = Some(poa_botanix_clients);
        }

        // // =================== RPC NODES ================== //
        if create_test_config.create_rpc_nodes {
            let (mut rpc_nodes, tx) = create_rpc_nodes(self.global_context.clone()).await?;
            let build_command_authorities_list = Arc::new(self.local_context.authorities.clone());
            let mut spawned_rpc_processes = vec![];

            it_info_print!("Starting rpc nodes ...");
            let mut rpc_botanix_clients = vec![];
            for (index, rpc_node) in rpc_nodes.iter_mut() {
                it_info_print!("Starting rpc node", index);
                let build_command_authorities_list = Arc::clone(&build_command_authorities_list);

                // get a clone of all poa nodes (already done in the prev. step)
                let poa_nodes_clone = self
                    .local_context
                    .poa_nodes
                    .clone()
                    .map(|poas| poas.values().cloned().collect::<Vec<FederationMemberTestConfig>>())
                    .unwrap_or_default();

                // spawn rpc node as a process
                spawned_rpc_processes
                    .push(rpc_node.spawn_service(build_command_authorities_list, poa_nodes_clone)?);

                // wait for two seconds in between processes start
                tokio::time::sleep(Duration::from_secs(2)).await;
            }

            // loop over the poa nodes and wait until they become initialized so the eth clients can
            // connect with them
            for (index, rpc_node) in rpc_nodes.iter_mut() {
                // create botanix client and await initialization
                let botanix_eth_client = loop {
                    match create_botanix_eth_client(rpc_node.rpc_port, rpc_node.ws_port).await {
                        Ok(client) => {
                            it_info_print!(
                                "Botanix client for rpc member {} just connected!",
                                index
                            );
                            break client;
                        }
                        Err(e) => {
                            it_warn_print!(
                                "Failed to create botanix client for rpc member",
                                index,
                                e
                            );
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                };
                rpc_node.botanix_eth_client = Some(botanix_eth_client.clone());
                rpc_botanix_clients.push(botanix_eth_client);
                it_info_print!("Botanix client created for poa member {}", index);

                // await initialization
                rpc_node.await_initialization()?;
            }

            // update local context
            self.local_context.rpc_processes = Some(spawned_rpc_processes);
            self.local_context.rpc_nodes = Some(rpc_nodes);
            self.local_context.rpc_notification = Some(tx);
            self.local_context.rpc_eth_providers = Some(rpc_botanix_clients);
        }

        Ok(())
    }
}

impl ConsensusIntegrationTestSuite {
    pub fn new(timeout: Duration, global_context: Arc<GlobalContext>) -> Self {
        Self {
            timeout,
            global_context,
            outcomes: Default::default(),
            local_context: LocalContext {
                btc_processes: None,
                poa_nodes: None,
                poa_notification: None,
                poa_eth_providers: None,
                poa_processes: None,
                btc_server_clients: None,
                rpc_nodes: None,
                rpc_notification: None,
                rpc_eth_providers: None,
                rpc_processes: None,
                cometbft_nodes: None,
                cometbft_nodes_syncing: None,
                cometbft_notification: None,
                cometbft_lightclients: None,
                cometbft_processes: None,
                bitcoind_node: None,
                bitcoind_notification: None,
                bitcoind_process: None,
                authorities: vec![],
            },
        }
    }
}
