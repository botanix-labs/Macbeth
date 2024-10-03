extern crate tracing;
use anyhow::{Context, Result};
use reth_tracing::{
    tracing_subscriber::filter::LevelFilter, LayerInfo, LogFormat, RethTracer, Tracer,
};
use std::sync::Arc;
use test_suite::{
    config::CliArgs, context::GlobalContext, it_error_print, it_info_print, server::TestServer,
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

    let result = tokio::spawn(async move { suite_test_server.start(test_to_run).await });
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
