use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
};

use anyhow::Result;
use btc::Utxo;
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
    /// Exports all UTXOs for the federation's known addresses to a file
    Export {
        /// The path to the file to export UTXOs to. If not provided, will print to stdout.
        #[clap(short, long)]
        output: Option<PathBuf>,
    },
    /// Compares the local UTXO set against an imported file
    Compare {
        /// The path to the file to import UTXOs from.
        #[clap(short, long)]
        input: PathBuf,
    },
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
            UtxoCommands::Export { output } => {
                let btc_client = BtcClient::new(&config)?;
                let utxos = btc_client.get_utxos().await?;
                let utxos_json = serde_json::to_string_pretty(&utxos)?;
                if let Some(output_path) = output {
                    let mut file = File::create(output_path)?;
                    file.write_all(utxos_json.as_bytes())?;
                } else {
                    println!("{}", utxos_json);
                }
            }
            UtxoCommands::Compare { input } => {
                let btc_client = BtcClient::new(&config)?;
                let local_utxos = btc_client.get_utxos().await?;
                let local_utxos: HashSet<Utxo> = local_utxos.into_iter().collect();

                let mut file = File::open(input)?;
                let mut contents = String::new();
                file.read_to_string(&mut contents)?;
                let remote_utxos: Vec<Utxo> = serde_json::from_str(&contents)?;
                let remote_utxos: HashSet<Utxo> = remote_utxos.into_iter().collect();

                let in_local_not_remote: Vec<_> =
                    local_utxos.difference(&remote_utxos).collect();
                let in_remote_not_local: Vec<_> =
                    remote_utxos.difference(&local_utxos).collect();

                if in_local_not_remote.is_empty() && in_remote_not_local.is_empty() {
                    println!("UTXO sets are identical.");
                } else {
                    if !in_local_not_remote.is_empty() {
                        println!("UTXOs present locally but not in remote set:");
                        println!("{:#?}", in_local_not_remote);
                    }
                    if !in_remote_not_local.is_empty() {
                        println!("UTXOs present in remote set but not locally:");
                        println!("{:#?}", in_remote_not_local);
                    }
                }
            }
        },
    }

    Ok(())
} 