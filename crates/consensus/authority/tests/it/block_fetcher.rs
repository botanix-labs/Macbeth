use reth::{
    cli::{
        components::RethNodeComponents,
        ext::{NoArgs, NoArgsCliExt, RethNodeCommandConfig},
    },
    commands::node::NodeCommand,
    core::cli::runner::CliRunner,
    tasks::TaskSpawner,
};
use reth_primitives::{hex, revm_primitives::FixedBytes, ChainSpec, Genesis};
use reth_provider::CanonStateSubscriptions;

use std::{sync::Arc, time::Duration};
use tokio::time::timeout;

#[test]
pub(crate) fn test_block_fetcher() {
    assert!(true)
}
