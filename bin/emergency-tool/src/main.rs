use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod btc;
mod config;

use btc::BtcClient;
use config::Config;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manages UTXOs
    Utxo {
        #[command(subcommand)]
        command: UtxoCommands,
    },
}

#[derive(Subcommand)]
pub enum UtxoCommands {
    /// Lists all UTXOs for the federation's known addresses
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config: Config = confy::load("botanix-emergency-tool", None)?;

    let cli = Cli::parse();

    match cli.command {
        Commands::Utxo { command } => match command {
            UtxoCommands::List => {
                let btc_client = BtcClient::new(&config)?;
                let utxos = btc_client.get_utxos().await?;
                println!("{:#?}", utxos);
            }
        },
    }

    Ok(())
} 