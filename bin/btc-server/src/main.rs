use btc_server::{
    config::{CliConfig, TomlConfig},
    App, shutdown::stop_signal,  
};

use clap::Parser;
use log::{info, error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Trace)
        .filter_module("sled::", log::LevelFilter::Info)
        .init();

    let cli_config = CliConfig::parse();
    let toml_config = TomlConfig::new(cli_config.config_path).await.expect("valid config provided");
    let app_config = toml_config.app;
    let grpc_config = toml_config.grpc;

    // setup the grpc server
    let btc_server = App::new(app_config.clone())?;

    // run grpc server in the background
    let grpc_stop_tx = match btc_server.serve_async(&grpc_config).await {
        Ok(s) => {
            info!("Grpc server: started successfully on {:?}", app_config.address);
            info!("Grpc server: waiting for a shutdown signal...");
            Some(s)
        }
        Err(err) => {
            error!("Grpc server: Join Error {}", err.to_string());
            None
        }
    };

    // spawn terminate handlers routine
    let grpc_join_handle = tokio::spawn(stop_signal(grpc_stop_tx));

    // block and wait for a shutdown signal to terminate
    let _ = tokio::join!(grpc_join_handle);

    Ok(())
}
