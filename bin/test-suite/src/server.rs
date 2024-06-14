use crate::{
    context::GlobalContext,
    it_info_print,
    suite::{consensus::ConsensusIntegrationTestSuite, Outcome, RunSuite, Suite},
};
use displaydoc::Display as DisplayDoc;
use reth_tracing::tracing::info;
use std::{sync::Arc, time::Duration};
use thiserror::Error;

#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Test Run Failed.
    TestRunFailed,
    /// Test Run Stopped
    TestRunStopped,
}

pub struct TestServer {
    context: Arc<GlobalContext>,
}

impl TestServer {
    pub fn new(context: Arc<GlobalContext>) -> Self {
        Self { context }
    }

    pub async fn start(
        mut self,
        stop_tx: tokio::sync::broadcast::Receiver<()>,
        test_to_run: String,
    ) -> Result<(), Error> {
        info!("Starting test instance...");
        let result = self.run(stop_tx, test_to_run).await;
        result
    }

    async fn run(
        &mut self,
        mut stop_tx: tokio::sync::broadcast::Receiver<()>,
        test_to_run: String,
    ) -> Result<(), Error> {
        let mut suits_to_run: Vec<Box<dyn Suite>> = vec![];
        let mut test_suite = self.create_consensus_test_suite();

        // match self.context.run_suite {
        //     RunSuite::All => {
        //         suits_to_run.push(self.create_consensus_test_suite());
        //     }
        //     RunSuite::Consensus => {
        //         suits_to_run.push(self.create_consensus_test_suite());
        //     }
        // }

        tokio::select! {
            _ = stop_tx.recv() => {
                it_info_print!(">>>> Term Sig received.");
                test_suite.destroy_context().await;
                // for suite in suits_to_run.iter_mut() {
                //     suite.destroy_context().await;
                //     info!(">>>> Destroyed test context for {:?}", suite.name());
                // }
                return Err(Error::TestRunStopped);
            },
            res = async {
                    let outcomes = test_suite.run(test_to_run).await;
                    // if any of them failed return error
                    if outcomes.iter().any(|outcome| outcome == &Outcome::Failed) {
                        return Err(Error::TestRunFailed);
                    }
                Ok(())
            } => { return res; },
        }
    }

    fn create_consensus_test_suite(&self) -> Box<dyn Suite> {
        info!(">>>> Starting censensus integration test suite...");
        let tests_timeout = Duration::from_millis(self.context.timeout);
        let test_suite = ConsensusIntegrationTestSuite::new(tests_timeout, self.context.clone());
        Box::new(test_suite)
    }
}
