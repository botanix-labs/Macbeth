use anyhow::{anyhow, Context, Result};
use argh::{self, FromArgs};
use std::{sync::Arc, time::Duration};
use test_suite::{
    config::Config, context::Context as ResourcesContext, server::TestServer, suite::RunSuite,
};
use tokio::{
    signal::unix::{signal, SignalKind},
    sync::broadcast,
};
use tracing::info;
use tracing_subscriber::fmt::format::FmtSpan;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    // init config
    dotenv::dotenv().ok();
    let args: Args = argh::from_env();
    info!("{:?}", args.config);

    // set up log filter to be used by tracing
    let log_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "test_suite=info".to_string());

    tracing_subscriber::fmt().with_env_filter(log_filter).with_span_events(FmtSpan::CLOSE).init();

    // init config
    let mut config = Config::new(args.config).await.context("Failed to load config")?;

    // update config using envs
    config.from_envs();

    tracing::info!("Configuration loaded successfully",);

    // create the shared resources
    let resources_ctx = Arc::new(ResourcesContext::new(args.dry_run));

    // create the test server instance
    let timeout = Duration::from_millis(args.timeout);
    let suite_test_server = TestServer::new(args.run_suite, timeout, resources_ctx.clone());

    // stop signal for suite
    let (stop_tx, stop_rx) = broadcast::channel(1);

    // spawn terminate handlers routine
    tokio::spawn(stop_signal(stop_tx, resources_ctx));

    let result = tokio::spawn(async move { suite_test_server.start(stop_rx).await });
    result
        .await
        .context("Failed to read test server result")?
        .map(|()| info!("Testing complete."))
        .map_err(|err| anyhow!("Testing failed: {}", err))?;

    Ok(())
}

async fn stop_signal(stop_tx: broadcast::Sender<()>, _resources_ctx: Arc<ResourcesContext>) {
    let mut sigint = signal(SignalKind::interrupt()).expect("shutdown_listener");
    let mut sigterm = signal(SignalKind::terminate()).expect("shutdown_listener");
    tokio::select! {
        _ = sigint.recv() => {
            tracing::info!("Received SIGINT ...");
            let _ = stop_tx.send(());
        }
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM ...");
            let _ = stop_tx.send(());
        }
    }
}

/// Test Suite Service
#[derive(FromArgs)]
struct Args {
    /// path to the config file
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
