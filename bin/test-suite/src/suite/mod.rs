pub mod consensus;
use async_trait::async_trait;
use strum_macros::{AsRefStr, EnumString};

#[async_trait]
pub trait Suite {
    async fn run(&mut self) -> Outcome;
    async fn create_context(&mut self);
    async fn destroy_context(&mut self);
}

#[derive(Clone, Copy, Debug, PartialEq, AsRefStr, EnumString)]
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
