pub mod consensus;
use async_trait::async_trait;
use strum_macros::{AsRefStr, EnumString};

use self::consensus::CreateTestConfig;

#[async_trait]
pub trait Suite: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn run(&mut self, test_to_run: String) -> Vec<Outcome>;
    async fn create_new_context(&mut self, create_test_config: CreateTestConfig);
    async fn destroy_context(&mut self);
    fn set_panic_hook(&self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, AsRefStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum RunSuite {
    All,
    Consensus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
}

impl Default for Outcome {
    fn default() -> Self {
        Outcome::Passed
    }
}
