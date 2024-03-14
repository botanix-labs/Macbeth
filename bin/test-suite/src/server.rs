use crate::{
    context::Context,
    suite::{consensus::ConsensusIntegrationTestSuite, Outcome, RunSuite, Suite},
};
use displaydoc::Display as DisplayDoc;
use std::{sync::Arc, time::Duration};
use thiserror::Error;
use tracing::info;

#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Test Run Failed.
    TestRunFailed,
    /// Test Run Stopped
    TestRunStopped,
}

pub struct TestServer {
    suite: RunSuite,
    timeout: Duration,
    context: Arc<Context>,
}

impl TestServer {
    pub fn new(suite: RunSuite, timeout: Duration, context: Arc<Context>) -> Self {
        Self { suite, timeout, context }
    }

    pub async fn start(
        mut self,
        stop_tx: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), Error> {
        info!("Starting test instance...");
        let result = self.run(stop_tx).await;
        result
    }

    async fn run(
        &mut self,
        mut stop_tx: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), Error> {
        tokio::select! {
            _ = stop_tx.recv() => {
                return Err(Error::TestRunStopped);
            },
            res = async {
                match self.suite {
                    RunSuite::All => {
                        self.run_consensus_integration_test_suite().await
                    }
                    RunSuite::Consensus => {
                        self.run_consensus_integration_test_suite().await
                    }
                }
            } => { res },
        }
    }

    async fn run_consensus_integration_test_suite(&mut self) -> Result<(), Error> {
        info!(">>>> Starting censensus integration test suite...");
        let mut test_suite = ConsensusIntegrationTestSuite::new(self.timeout, self.context.clone());

        match test_suite.run().await {
            Outcome::Passed => Ok(()),
            Outcome::Failed => Err(Error::TestRunFailed),
        }
    }
}
