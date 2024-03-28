use self::frost::btc_server::SpawnedBtcServer;
use super::{Outcome, Suite};
use crate::{
    config::Config,
    context::Context,
    run_test,
    suite::consensus::frost::btc_server::{clean_db, spawn_n_btc_servers},
};
use async_trait::async_trait;
use reth_tracing::tracing::error;
use std::{panic, process::Command, sync::Arc, time::Duration};

// scopes
mod frost;
mod poa;

pub struct ConsensusIntegrationTestSuite {
    pub timeout: Duration,
    pub global_context: Arc<Context>,
    pub outcome: Outcome,
    pub local_context: LocalContext,
    pub config: Config,
}

pub struct LocalContext {
    pub btc_servers: Option<Vec<SpawnedBtcServer>>,
}

#[async_trait]
impl Suite for ConsensusIntegrationTestSuite {
    async fn run(&mut self) -> Outcome {
        self.set_panic_hook();

        // dkg tests
        //run_test!(self, frost::dkg::dkg_flow);
        // signing tests
        //run_test!(self, frost::signing::test_many_inputs_signing);
        // eoa tests
        //run_test!(self, poa::block_builder::poa_eoa);
        // frost dkg tests

        run_test!(self, poa::frost_dkg::poa_frost_dkg);
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

        // set the panic hook so it kills them whenever activated
        std::panic::set_hook(Box::new(move |panic_info| {
            error!("Test suite panicked {:?}", panic_info);
            for process_id in child_processes_to_kill.iter() {
                // Send a termination signal to the child process
                let _ = Command::new("kill")
                    .arg("-9") // Use SIGKILL for immediate termination
                    .arg(format!("{}", process_id))
                    .output();
            }
        }));
    }

    async fn create_context(&mut self) {
        // cleanup if needed
        // create new context
        self.local_context.btc_servers = Some(spawn_n_btc_servers(3, self.config.clone()));

        // let servers come up
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
    }

    async fn destroy_context(&mut self) {
        if let Some(btc_servers) = self.local_context.btc_servers.as_mut() {
            for btc_server in btc_servers.iter_mut() {
                let _ = btc_server.child_process.kill().await;
            }
            // Remove db dirs
            clean_db(&btc_servers);
        }
    }
}

impl ConsensusIntegrationTestSuite {
    pub fn new(timeout: Duration, global_context: Arc<Context>, config: Config) -> Self {
        Self {
            timeout,
            global_context,
            outcome: Default::default(),
            local_context: LocalContext { btc_servers: None },
            config,
        }
    }
}
