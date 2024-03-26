
#[macro_use]
extern crate tracing;

use anyhow::{anyhow, Context, Result};
use argh::{self, FromArgs};
use ethers::contract::Abigen;
use std::{sync::Arc, time::Duration};
use test_suite::{
    config::Config, context::Context as ResourcesContext, it_info_print, server::TestServer,
    suite::RunSuite,
};
use tokio::{
    signal::unix::{signal, SignalKind},
    sync::broadcast,
};
//use tracing_subscriber::fmt::format::FmtSpan;
use reth_tracing::{
    tracing_subscriber::filter::LevelFilter, LayerInfo, LogFormat, RethTracer, Tracer,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    // generate contract abi
    Abigen::new("MintContract", "mint_contract_abi.json")
        .expect("Error reading mint contract json abi")
        .generate()
        .expect("Error generating mint contract rust defintions")
        .write_to_file("./src/mint_contract_abi.rs")
        .expect("Error writing mint contract rust file");

    // init config
    dotenv::dotenv().ok();
    let args: Args = argh::from_env();

    // set up log filter to be used by tracing
    let log_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "test_suite=info".to_string());

    let _ = RethTracer::new()
        .with_stdout(LayerInfo::new(
            LogFormat::Terminal,
            LevelFilter::INFO.to_string(),
            log_filter,
            Some("always".to_string()),
        ))
        .init();

    // init config
    let mut config = Config::new(args.config).await.context("Failed to load config")?;

    // update config using envs
    config.from_envs();

    it_info_print!("Configuration loaded successfully");

    // create the shared resources
    let resources_ctx = Arc::new(ResourcesContext::new(args.dry_run));

    // create the test server instance
    let timeout = Duration::from_millis(args.timeout);
    let suite_test_server = TestServer::new(args.run_suite, timeout, resources_ctx.clone(), config);

    // stop signal for suite
    let (stop_tx, stop_rx) = broadcast::channel(1);

    // spawn terminate handlers routine
    tokio::spawn(stop_signal(stop_tx, resources_ctx));

    let result = tokio::spawn(async move { suite_test_server.start(stop_rx).await });
    result
        .await
        .context("Failed to read test server result")?
        .map(|()| it_info_print!("Testing complete."))
        .map_err(|err| anyhow!("Testing failed: {}", err))?;

    Ok(())
}

async fn stop_signal(stop_tx: broadcast::Sender<()>, _resources_ctx: Arc<ResourcesContext>) {
    let mut sigint = signal(SignalKind::interrupt()).expect("shutdown_listener");
    let mut sigterm = signal(SignalKind::terminate()).expect("shutdown_listener");
    tokio::select! {
        _ = sigint.recv() => {
            it_info_print!("Received SIGINT ...");
            let _ = stop_tx.send(());
        }
        _ = sigterm.recv() => {
            it_info_print!("Received SIGTERM ...");
            let _ = stop_tx.send(());
        }
    }
}

/// Test Suite Service
#[derive(FromArgs)]
struct Args {
    /// path to the toml config file
    #[argh(option, short = 'c')]
    config: String,
    /// suite of tests to run: Consensus|all (default: all)
    #[argh(option, short = 'r', from_str_fn(parse_suite), default = "RunSuite::Consensus")]
    run_suite: RunSuite,
    /// individual test timeout in milliseconds (default: 20000)
    #[argh(option, short = 't', default = "20_000")]
    timeout: u64,
    /// dry run to perform (default: false)
    #[argh(option, short = 'd', default = "false")]
    dry_run: bool,
}

pub fn parse_suite(value: &str) -> Result<RunSuite, String> {
    value.parse().map_err(|_| format!("Failed to parse RunSuite: {}", value))
}
