use self::frost::btc_server::{SpawnedBtcServer, BTC_SERVER_START_PORT};
use super::{Outcome, Suite};
use crate::{
    context::GlobalContext,
    run_test,
    suite::consensus::frost::btc_server::{clean_db, spawn_n_btc_servers},
};
use async_trait::async_trait;
use port_killer::kill;
use reth_tracing::tracing::error;
use std::{panic, path::PathBuf, process::Command, sync::Arc, time::Duration};
use tracing::{info, warn};
// scopes
mod frost;

fn kill_child_processes_at_port(index: u16) {
    // kill btc server processes
    let btc_server_port = BTC_SERVER_START_PORT + index;
    match kill(btc_server_port) {
        Ok(pid) => {
            if pid {
                info!(
                    "Sucessfully killed btc-server process on port process on port {:?}",
                    btc_server_port
                );
            } else {
                warn!("Unable to kill btc-server process on port {:?}", btc_server_port);
            }
        }
        Err(err) => {
            error!(
                "Error attempting to kill btc-server process on port {:?} -> {:?}",
                btc_server_port, err
            );
        }
    }
}

pub struct ConsensusIntegrationTestSuite {
    pub timeout: Duration,
    pub global_context: Arc<GlobalContext>,
    pub outcome: Outcome,
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

    async fn run(&mut self) -> Outcome {
        self.set_panic_hook();

        // dkg tests
        run_test!(self, frost::test_dkg::dkg_flow);
        // // signing tests
        run_test!(self, frost::test_signing::test_many_inputs_signing);
        // // eoa tests
        run_test!(self, frost::test_block_builder::block_builder);
        // utxo commitment test
        run_test!(self, frost::test_utxo_commitment::test_utxo_commitment);
        // frost e2e tests
        run_test!(self, frost::test_frost_e2e::frost_e2e_stable);
        run_test!(self, frost::test_frost_e2e_edge_cases::frost_e2e_failed_signing_round);

        self.outcome
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

    async fn create_context(&mut self) {
        if let Some(btc_servers) = self.local_context.btc_servers.as_mut() {
            // kill all btc server processes
            for (_, btc_server) in btc_servers.iter_mut().enumerate() {
                let _ = btc_server.child_process.kill().await;
            }
            // Remove db dirs
            clean_db(btc_servers);
        }

        // kill processes at designated ports
        (0..self.global_context.instances).for_each(|i| {
            kill_child_processes_at_port(i);
        });

        // let old context be fully destroyed
        tokio::time::sleep(Duration::from_secs(15)).await;

        // create new context
        self.local_context.btc_servers = Some(spawn_n_btc_servers(self.global_context.clone()));

        // let servers come up
        tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;
    }

    async fn destroy_context(&mut self) {
        if let Some(btc_servers) = self.local_context.btc_servers.as_mut() {
            for (index, btc_server) in btc_servers.iter_mut().enumerate() {
                let _ = btc_server.child_process.kill().await;
                kill_child_processes_at_port(index as u16);
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
            outcome: Default::default(),
            local_context: LocalContext { btc_servers: None },
        }
    }
}
