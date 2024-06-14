use self::common::poa_node::{FederationMemberTestConfig, Notifications};

use super::{Outcome, Suite};
use crate::{
    context::GlobalContext,
    it_info_print, it_warn_print, run_test,
    suite::consensus::common::{
        btc_server::{clean_db, spawn_n_btc_servers, SpawnedBtcServer, BTC_SERVER_START_PORT},
        events::await_dkg,
        poa_node::create_poa_federation_members,
    },
};
use async_trait::async_trait;
use port_killer::kill;
use reth::CliRunner;
use reth_tracing::tracing::error;
use std::{
    collections::HashMap, panic, path::PathBuf, process::Command, sync::Arc, time::Duration,
};
use tracing::{info, warn};
// scopes
mod common;
mod frost;
mod invalid_transactions;
mod pbft;
mod rpc_node;

fn kill_process_at_port(port: u16) {
    // kill server process
    match kill(port) {
        Ok(pid) => {
            if pid {
                info!("Sucessfully killed server process on port process on port {:?}", port);
            } else {
                warn!("Unable to kill server process on port {:?}", port);
            }
        }
        Err(err) => {
            error!("Error attempting to kill server process on port {:?} -> {:?}", port, err);
        }
    }
}

pub struct ConsensusIntegrationTestSuite {
    pub timeout: Duration,
    pub global_context: Arc<GlobalContext>,
    pub outcomes: Vec<Outcome>,
    pub local_context: LocalContext,
}
pub struct LocalContext {
    pub btc_servers: Option<Vec<SpawnedBtcServer>>,
    pub poa_nodes: Option<HashMap<u16, FederationMemberTestConfig>>,
    pub poa_notification: Option<tokio::sync::broadcast::Sender<Notifications>>,
}

pub struct CreateTestConfig {
    pub should_create_poa_nodes: bool,
}

impl Default for CreateTestConfig {
    fn default() -> Self {
        Self { should_create_poa_nodes: true }
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
                CreateTestConfig { should_create_poa_nodes: false },
                frost::test_dkg::dkg_flow
            ),
            "many_inputs_signing" => run_test!(
                self,
                CreateTestConfig { should_create_poa_nodes: false },
                frost::test_signing::test_many_inputs_signing
            ),
            "utxo_commitment" => run_test!(
                self,
                CreateTestConfig { should_create_poa_nodes: false },
                frost::test_utxo_commitment::test_utxo_commitment
            ),
            "block_builder" => {
                run_test!(self, Default::default(), frost::test_block_builder::block_builder)
            },
            "frost_e2e_stable" => {
                run_test!(self, Default::default(), frost::test_frost_e2e::frost_e2e_stable)
            },
            "frost_e2e_failed_signing_disconnect" => run_test!(
                self,
                Default::default(),
                frost::test_frost_e2e_signing_disconnect::frost_e2e_failed_signing_disconnect
            ),
            // TODO
            // "rpc_node" => run_test!(self, Default::default(), rpc_node::test_rpc_node::test_rpc_node),
            "invalid_pegin" => {
                run_test!(self, Default::default(), invalid_transactions::test_invalid_pegin::invalid_pegin)
            },
            "invalid_pegout" => {
                run_test!(self, Default::default(), invalid_transactions::test_invalid_pegout::invalid_pegout)
            },
            "test_mempool_gossip" => {
                run_test!(self, Default::default(), frost::test_mempool_gossip::test_mempool_gossip)
            },  
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
            .btc_servers
            .as_ref()
            .map(|btc_servers| {
                btc_servers
                    .iter()
                    .map(|server| server.child_process.id())
                    .collect::<Vec<Option<u32>>>()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|process_id| process_id)
            .collect::<Vec<u32>>();

        let dbs_to_delete = self
            .local_context
            .btc_servers
            .as_ref()
            .map(|btc_servers| {
                btc_servers.iter().map(|server| server.db_path.clone()).collect::<Vec<PathBuf>>()
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

    async fn create_new_context(&mut self, create_test_config: CreateTestConfig) {
        self.local_context.btc_servers = Some(spawn_n_btc_servers(self.global_context.clone()));
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
                    .btc_servers
                    .as_ref()
                    .and_then(|servers| servers.iter().nth(instance as usize).map(|val| val.port))
                    .expect("btc server port");
                match client::BtcServerClient::connect(format!("http://localhost:{}", port)).await {
                    Ok(_) => {
                        it_info_print!("Connected to btc server at port {:?}", port);
                        successes += 1;
                    }
                    Err(e) => {
                        it_warn_print!(
                            "Failed to connect to btc server at port {:?} -> {:?}",
                            port,
                            e
                        );
                    }
                }

                tries -= 1;
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
        it_info_print!("Connected to all btc servers");
        // generate test fed members poa nodes
        let (mut test_fed_members, tx) = create_poa_federation_members(
            self.global_context.clone(),
            self.local_context.btc_servers.as_ref(),
        )
        .await;

        // run all poa nodes in the background
        if create_test_config.should_create_poa_nodes {
            let mut rx = tx.subscribe();
            for (_index, fed_member_config) in test_fed_members.iter() {
                it_info_print!("Starting poa node", _index);
                let fed_member_config = fed_member_config.clone();
                // Need to spawn a seperate thread due to nested runtime issues
                let _ = std::thread::spawn(move || {
                    let (fed_member_command, _chain_spec) = fed_member_config.build_command();
                    let runner = CliRunner::default();
                    runner.run_command_until_exit(|ctx| fed_member_command.execute(ctx)).unwrap();
                });

                // wait for one second in between processes start
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            await_dkg(&mut test_fed_members, &mut rx).await;

            self.local_context.poa_nodes = Some(test_fed_members);
            self.local_context.poa_notification = Some(tx);
        }
    }

    async fn destroy_context(&mut self) {
        it_info_print!("Destroying test suite context");
        if let Some(btc_servers) = self.local_context.btc_servers.as_mut() {
            for (index, btc_server) in btc_servers.iter_mut().enumerate() {
                let _ = btc_server.child_process.kill().await;
                kill_process_at_port(BTC_SERVER_START_PORT + index as u16);
            }
            // Remove db dirs
            clean_db(btc_servers);
        }
        self.local_context.btc_servers = None;
        self.local_context.poa_nodes = None;
        self.local_context.poa_notification = None;
    }
}

impl ConsensusIntegrationTestSuite {
    pub fn new(timeout: Duration, global_context: Arc<GlobalContext>) -> Self {
        Self {
            timeout,
            global_context,
            outcomes: Default::default(),
            local_context: LocalContext {
                btc_servers: None,
                poa_nodes: None,
                poa_notification: None,
            },
        }
    }
}
