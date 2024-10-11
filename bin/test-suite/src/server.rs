use crate::{
    context::GlobalContext,
    it_info_print,
    shutdown::StopHandle,
    suite::{consensus::ConsensusIntegrationTestSuite, Outcome, Suite},
};
use displaydoc::Display as DisplayDoc;
use reth_tracing::tracing::info;
use std::{sync::Arc, time::Duration};
use thiserror::Error;

#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Test Run Failed.
    TestRunFailed,
    /// Test Run Stopped.
    TestRunStopped,
}

pub struct TestServer {
    context: Arc<GlobalContext>,
}

impl TestServer {
    pub fn new(context: Arc<GlobalContext>) -> Self {
        Self { context }
    }

    pub async fn start(mut self, test_to_run: String) -> Result<(), Error> {
        info!("Starting test instance...");
        let result = self.run(test_to_run).await;
        result
    }

    async fn run(&mut self, test_to_run: String) -> Result<(), Error> {
        let mut stop_handle = StopHandle::new();
        stop_handle.spawn_signal_listener();
        let mut test_suite = self.create_consensus_test_suite();

        tokio::select! {
            shutdown = stop_handle.wait_for_signal() => {
                if shutdown {
                    it_info_print!("Shutdown signal received. Destroying local context...");
                    test_suite.destroy_local_context().await;
                    return Err(Error::TestRunStopped);
                }
                Ok(())
            },
            res = async {
                    // TODO this will always be a vec of one element
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
