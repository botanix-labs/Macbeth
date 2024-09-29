use self::common::{
    botanix_client::BotanixEthClient,
    poa_node::{FederationMemberTestConfig, Notifications},
    rpc_node::NonFederationMemberTestConfig,
};

use super::{Outcome, Suite};
use crate::{
    context::GlobalContext,
    it_info_print, it_warn_print, run_test,
    suite::consensus::common::{
        btc_server::{
            clean_db, spawn_n_btc_server_processes, SpawnedBtcServerProcess, BTC_SERVER_START_PORT,
        },
        events::await_dkg,
        poa_node::create_poa_federation_members,
        rpc_node::create_rpc_node,
    },
};
use anyhow::Context;
use async_trait::async_trait;
use client::BtcServerClient;
use common::{
    kill_process_at_port, poa_node::SpawnedPoaServerProcess, rpc_node::SpawnedRpcServerProcess,
    BOTANIX_FEE_RECEIPIENT,
};
use reth_tracing::tracing::error;
use std::{
    collections::{HashMap, HashSet},
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
mod pbft;
mod rpc_node;
pub struct ConsensusIntegrationTestSuite {
    pub timeout: Duration,
    pub global_context: Arc<GlobalContext>,
    pub outcomes: Vec<Outcome>,
    pub local_context: LocalContext,
}
pub struct LocalContext {
    // btc
    pub btc_processes: Option<Vec<SpawnedBtcServerProcess>>,
    pub btc_server_clients: Option<Vec<BtcServerClient<Channel>>>,
    // poa
    pub poa_processes: Option<Vec<SpawnedPoaServerProcess>>,
    pub poa_nodes: Option<HashMap<u16, FederationMemberTestConfig>>,
    pub poa_notification: Option<tokio::sync::broadcast::Sender<Notifications>>,
    // rpc
    pub rpc_processes: Option<Vec<SpawnedRpcServerProcess>>,
    pub rpc_node: Option<Vec<NonFederationMemberTestConfig>>,
    pub rpc_notification: Option<tokio::sync::broadcast::Sender<Notifications>>,
    pub authorities: Vec<secp256k1::PublicKey>,
    pub botanix_fee_recipient: String,
    // Only available if poa nodes are being created for the test
    pub eth_providers: Option<Vec<BotanixEthClient>>,
}

pub struct CreateTestConfig {
    pub should_create_poa_nodes: bool,
    pub should_create_rpc_node: bool,
}

impl Default for CreateTestConfig {
    fn default() -> Self {
        Self { should_create_poa_nodes: true, should_create_rpc_node: false }
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
                CreateTestConfig { should_create_poa_nodes: false, should_create_rpc_node: false },
                frost::test_dkg::dkg_flow
            ),
            "many_inputs_signing" => run_test!(
                self,
                CreateTestConfig { should_create_poa_nodes: false, should_create_rpc_node: false },
                frost::test_signing::test_many_inputs_signing
            ),
            "utxo_commitment" => run_test!(
                self,
                CreateTestConfig { should_create_poa_nodes: false, should_create_rpc_node: false },
                frost::test_utxo_commitment::test_utxo_commitment
            ),
            // TODO comment these back in as we fix the test suite
            // "block_builder" => {
            //     run_test!(self, Default::default(), frost::test_block_builder::block_builder)
            // }
            // "batch_pegins" => {
            //     run_test!(self, Default::default(), frost::test_batch_pegins::batch_pegins)
            // }
            // "utxo_sync" => {
            //     run_test!(self, Default::default(), frost::test_utxo_sync::utxo_sync)
            // }
            // "frost_e2e_stable" => {
            //     run_test!(self, Default::default(), frost::test_frost_e2e::frost_e2e_stable)
            // }
            // "frost_e2e_failed_signing_disconnect" => run_test!(
            //     self,
            //     Default::default(),
            //     frost::test_frost_e2e_signing_disconnect::frost_e2e_failed_signing_disconnect
            // ),
            // "e2e_peer_disconnect" => run_test!(
            //     self,
            //     Default::default(),
            //     frost::test_e2e_peer_disconnect::e2e_peer_disconnect,
            // ),
            // "test_edh_size_limit" => {
            //     run_test!(self, Default::default(),
            // frost::test_edh_size_limit::test_edh_size_limit,) }
            // "rpc_node" => {
            //     run_test!(
            //         self,
            //         CreateTestConfig {
            //             should_create_poa_nodes: true,
            //             should_create_rpc_node: true
            //         },
            //         rpc_node::test_rpc_node::test_rpc_node
            //     )
            // }
            // "invalid_pegin" => {
            //     run_test!(
            //         self,
            //         Default::default(),
            //         invalid_transactions::test_invalid_pegin::invalid_pegin
            //     )
            // }
            // "invalid_pegout" => {
            //     run_test!(
            //         self,
            //         Default::default(),
            //         invalid_transactions::test_invalid_pegout::invalid_pegout
            //     )
            // }
            // "test_mempool_gossip" => {
            //     run_test!(self, Default::default(),
            // frost::test_mempool_gossip::test_mempool_gossip) }
            _ => {
                panic!("Test not found");
            }
        };

        self.outcomes.clone()
    }

    fn set_panic_hook(&self) {
        // get all child processes which are to be killed
        let child_processes_to_kill = self
            .local_context
            .btc_processes
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
            .collect::<Vec<u32>>();

        let dbs_to_delete = self
            .local_context
            .btc_processes
            .as_ref()
            .map(|btc_processes| {
                btc_processes
                    .iter()
                    .map(|process| process.db_path.clone())
                    .collect::<Vec<PathBuf>>()
            })
            .unwrap_or_default();

        // set the panic hook so it kills them whenever activated
        std::panic::set_hook(Box::new(move |panic_info| {
            error!("Test suite panicked {:?}", panic_info);
            for process_id in &child_processes_to_kill {
                // Send a termination signal to the child process
                let _ = Command::new("kill")
                    .arg("-9") // Use SIGKILL for immediate termination
                    .arg(format!("{process_id}"))
                    .output();
            }
            // delete db leftovers
            for db_to_delete in &dbs_to_delete {
                let _ = std::fs::remove_dir_all(db_to_delete.clone());
            }
            std::process::exit(1);
        }));
    }

    async fn create_new_context(
        &mut self,
        create_test_config: CreateTestConfig,
    ) -> anyhow::Result<()> {
        self.local_context.btc_processes =
            Some(spawn_n_btc_server_processes(self.global_context.clone())?);
        // let btc servers come up
        tokio::time::sleep(Duration::from_secs(5)).await;
        // try to connect to each btc server before moving on
        let mut tries = 5;
        let mut successes = 0;
        loop {
            it_info_print!("Trying to connect to all btc servers");
            if successes == self.global_context.instances {
                break;
            }
            if tries == 0 {
                panic!("Failed to connect to all btc servers");
            }
            successes = 0;
            for instance in 0..self.global_context.instances {
                let port = self
                    .local_context
                    .btc_processes
                    .as_ref()
                    .and_then(|processes| {
                        processes.iter().nth(instance as usize).map(|val| val.port)
                    })
                    .context("could not find btc server at instance index")?;
                match client::BtcServerClient::connect(format!("http://localhost:{}", port)).await {
                    Ok(_) => {
                        it_info_print!("Connected to btc server at port", port);
                        successes += 1;
                    }
                    Err(e) => {
                        it_warn_print!("Failed to connect to btc server at port", port, e);
                    }
                }

                tries -= 1;
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
        it_info_print!("Connected to all btc servers");
        // Save the clients in local context
        let mut btc_server_clients = vec![];
        for instance in 0..self.global_context.instances {
            let port = self
                .local_context
                .btc_processes
                .as_ref()
                .and_then(|processes| processes.iter().nth(instance as usize).map(|val| val.port))
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

        // generate test fed members poa nodes
        let (mut test_fed_members, tx, edh_authorities_list) = create_poa_federation_members(
            self.global_context.clone(),
            self.local_context.btc_processes.as_ref(),
            self.local_context.botanix_fee_recipient.clone(),
        )
        .await?;

        self.local_context.authorities = edh_authorities_list.clone();
        self.local_context.botanix_fee_recipient = BOTANIX_FEE_RECEIPIENT.to_string();

        let build_command_authorities_list = Arc::new(edh_authorities_list);

        // run all poa nodes in the background
        if create_test_config.should_create_poa_nodes {
            it_info_print!("Starting poa nodes");
            let mut rx = tx.subscribe();
            for (_index, fed_member_config) in test_fed_members.iter() {
                it_info_print!("Starting poa node", _index);
                let fed_member_config = fed_member_config.clone();
                let build_command_authorities_list = Arc::clone(&build_command_authorities_list);

                // spawn poa node as a process
                let spawned_child_process =
                    fed_member_config.spawn_service(build_command_authorities_list)?;

                // wait for one second in between processes start
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            await_dkg(&mut test_fed_members, &mut rx).await;

            // At this point all the btc servers should have the same aggregate key
            let mut keys = HashSet::new();
            for client in btc_server_clients.iter_mut() {
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

            let mut botanix_clients = vec![];
            for (index, fed_member_config) in test_fed_members.iter() {
                botanix_clients.push(fed_member_config.botanix_eth_client.clone());
                it_info_print!("Botanix client created for poa member {}", index);
            }

            self.local_context.poa_nodes = Some(test_fed_members);
            self.local_context.poa_notification = Some(tx);
            self.local_context.eth_providers = Some(botanix_clients);
        }

        if create_test_config.should_create_rpc_node {
            it_info_print!("Starting rpc node");
            let federation_members = self
                .local_context
                .poa_nodes
                .as_ref()
                .context("Expected poa nodes to be created")?;
            let (rpc_node, tx) = create_rpc_node(
                self.global_context.clone(),
                federation_members.clone(),
                self.local_context.botanix_fee_recipient.clone(),
            )
            .await?;

            let mut rpc_node_clone = rpc_node.clone();
            let spawned_child_process = rpc_node_clone.spawn_service(
                build_command_authorities_list,
                federation_members.clone().into_values().collect::<Vec<_>>(),
            )?;
            self.local_context.rpc_notification = Some(tx);
            self.local_context.rpc_node = Some(vec![rpc_node]);

            // wait for rpc node to start on thread
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        Ok(())
    }

    async fn destroy_context(&mut self) {
        it_info_print!("Destroying test suite context");
        if let Some(btc_processes) = self.local_context.btc_processes.as_mut() {
            for (index, btc_process) in btc_processes.iter_mut().enumerate() {
                let _ = btc_process.child_process.kill().await;
                kill_process_at_port(BTC_SERVER_START_PORT + index as u16);
            }
            // Remove db dirs
            clean_db(btc_processes);
        }
        self.local_context.btc_processes = None;
        self.local_context.btc_server_clients = None;
        self.local_context.poa_nodes = None;
        self.local_context.poa_processes = None;
        self.local_context.poa_notification = None;
        self.local_context.rpc_node = None;
        self.local_context.rpc_notification = None;
        self.local_context.rpc_processes = None;
        self.local_context.eth_providers = None;

        // allow a few seconds to pass after cleanup
        tokio::time::sleep(Duration::from_secs(5)).await;
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
                btc_server_clients: None,
                rpc_node: None,
                rpc_notification: None,
                authorities: vec![],
                botanix_fee_recipient: BOTANIX_FEE_RECEIPIENT.to_string(),
                eth_providers: None,
                poa_processes: None,
                rpc_processes: None,
            },
        }
    }
}
