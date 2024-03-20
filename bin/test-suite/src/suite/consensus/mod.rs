use self::frost::btc_server::SpawnedBtcServer;

use super::{Outcome, Suite};
use crate::{
    config::Config,
    context::Context,
    run_test,
    suite::consensus::frost::btc_server::{clean_db, spawn_n_btc_servers},
};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};

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
        // dkg tests
        run_test!(self, frost::dkg::dkg_flow);
        // signing tests
        run_test!(self, frost::signing::test_many_inputs_signing);
        // eoa tests
        //run_test!(self, poa::block_builder::poa_eoa);
        self.outcome
    }

    async fn create_context(&mut self) {
        // cleanup if needed
        // create new context
        self.local_context.btc_servers =
            Some(spawn_n_btc_servers(3, self.config.jwt_secrets_dir.clone()));

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
