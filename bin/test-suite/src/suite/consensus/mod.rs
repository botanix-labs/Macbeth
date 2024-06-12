use super::{Outcome, Suite};
use crate::{
    context::GlobalContext,
    run_test,
    suite::consensus::common::btc_server::{
        clean_db, spawn_n_btc_servers, SpawnedBtcServer, BTC_SERVER_START_PORT,
    },
};
use async_trait::async_trait;
use port_killer::kill;
use reth_tracing::tracing::error;
use std::{panic, path::PathBuf, process::Command, sync::Arc, time::Duration};
use tracing::{info, warn};

// scopes
mod common;
mod frost;
mod invalid_transactions;
mod pbft;
mod rpc_node;

fn kill_child_processes_at_port(port: u16) {
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
}

#[async_trait]
impl Suite for ConsensusIntegrationTestSuite {
    fn name(&self) -> &str {
        "ConsensusIntegrationTestSuite"
    }

    async fn run(&mut self) -> Vec<Outcome> {
        self.set_panic_hook();

        // dkg tests
        run_test!(self, frost::test_dkg::dkg_flow);
        // signing tests
        run_test!(self, frost::test_signing::test_many_inputs_signing);
        // eoa tests
        run_test!(self, frost::test_block_builder::block_builder);
        // utxo commitment test
        run_test!(self, frost::test_utxo_commitment::test_utxo_commitment);
        // frost e2e tests
        run_test!(self, frost::test_frost_e2e::frost_e2e_stable);
        run_test!(
            self,
            frost::test_frost_e2e_signing_disconnect::frost_e2e_failed_signing_disconnect
        );
        // rpc node tests
        run_test!(self, rpc_node::test_rpc_node::test_rpc_node);
        // run invalid transaction tests
        run_test!(self, invalid_transactions::test_invalid_pegin::invalid_pegin);
        run_test!(self, invalid_transactions::test_invalid_pegout::invalid_pegout);

        // pbft tests (WIP)
        //run_test!(self, pbft::test_pbft_disconnect::pbft_e2e_failed_disconnect);

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
        }));
    }

    async fn create_new_context(&mut self) {
        info!("Creating test suite context");
        if let Some(btc_servers) = self.local_context.btc_servers.as_mut() {
            // kill all btc server processes
            for (index, btc_server) in btc_servers.iter_mut().enumerate() {
                let _ = btc_server.child_process.kill().await;
                // kill processes at designated ports
                kill_child_processes_at_port(BTC_SERVER_START_PORT + index as u16);
            }
            // Remove db dirs
            clean_db(btc_servers);
        }

        let last_rpc_port = self.global_context.last_poa_node_rpc_port.lock().await;
        kill_child_processes_at_port(*last_rpc_port);
        drop(last_rpc_port);

        let last_authrpc_port = self.global_context.last_poa_node_authrpc_port.lock().await;
        kill_child_processes_at_port(*last_authrpc_port);
        drop(last_authrpc_port);

        let last_discovery_port = self.global_context.last_poa_node_discovery_port.lock().await;
        kill_child_processes_at_port(*last_discovery_port);
        drop(last_discovery_port);

        // let old context be fully destroyed
        tokio::time::sleep(Duration::from_secs(15)).await;

        // create new context
        self.local_context.btc_servers = Some(spawn_n_btc_servers(self.global_context.clone()).await);

        // let servers come up
        // try to connect to each btc server before moving on
        let mut tries = 5;
        let mut successes = 0;
        loop {
            info!("Trying to connect to all btc servers");
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
                        info!("Connected to btc server at port {:?}", port);
                        successes += 1;
                    }
                    Err(e) => {
                        warn!("Failed to connect to btc server at port {:?} -> {:?}", port, e);
                    }
                }

                tries -= 1;
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
            info!("Connected to all btc servers");
        }
    }

    async fn destroy_context(&mut self) {
        if let Some(btc_servers) = self.local_context.btc_servers.as_mut() {
            for (index, btc_server) in btc_servers.iter_mut().enumerate() {
                let _ = btc_server.child_process.kill().await;
                kill_child_processes_at_port(BTC_SERVER_START_PORT + index as u16);
            }
            // Remove db dirs
            clean_db(btc_servers);
        }
    }
}

impl ConsensusIntegrationTestSuite {
    pub fn new(timeout: Duration, global_context: Arc<GlobalContext>) -> Self {
        Self {
            timeout,
            global_context,
            outcomes: Default::default(),
            local_context: LocalContext { btc_servers: None },
        }
    }
}
