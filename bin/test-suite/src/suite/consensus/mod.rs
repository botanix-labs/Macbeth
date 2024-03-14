use super::{Outcome, Suite};
use crate::{context::Context, run_test};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};

// scopes
mod frost;
mod poa;

pub struct ConsensusIntegrationTestSuite {
    pub timeout: Duration,
    pub global_context: Arc<Context>,
    pub outcome: Outcome,
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
}

impl ConsensusIntegrationTestSuite {
    pub fn new(timeout: Duration, global_context: Arc<Context>) -> Self {
        Self { timeout, global_context, outcome: Default::default() }
    }
}
