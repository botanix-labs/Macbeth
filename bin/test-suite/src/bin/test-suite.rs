extern crate tracing;
use anyhow::{Context, Result};
use reth_tracing::{
    tracing_subscriber::filter::LevelFilter, LayerInfo, LogFormat, RethTracer, Tracer,
};
use std::sync::Arc;
use test_suite::{
    config::CliArgs, context::GlobalContext, it_error_print, it_info_print, server::TestServer,
};
use tokio::{
    signal::unix::{signal, SignalKind},
    sync::broadcast,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    // init config
    dotenv::dotenv().ok();
    let cli_args: CliArgs = argh::from_env();
    let test_to_run = cli_args.test_to_run.clone();

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

    it_info_print!("Configuration loaded successfully");

    // create the shared resources
    let resources_ctx =
        Arc::new(GlobalContext::new(cli_args).await.context("Failed to create global context")?);

    // create the test server instance
    let suite_test_server = TestServer::new(resources_ctx.clone());

    // stop signal for suite
    let (stop_tx, stop_rx) = broadcast::channel(1);

    // spawn terminate handlers routine
    tokio::spawn(stop_signal(stop_tx, resources_ctx));

    let result = tokio::spawn(async move { suite_test_server.start(stop_rx, test_to_run).await });
    match result.await.context("Failed to read test server result")? {
        Ok(_) => {
            it_info_print!("Testing complete.");
            std::process::exit(0);
        }
        Err(err) => {
            it_error_print!("Testing failed: {}", err);
            std::process::exit(1);
        }
    }
}

async fn stop_signal(stop_tx: broadcast::Sender<()>, _resources_ctx: Arc<GlobalContext>) {
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
