/// Module that defines the CLI commands
mod cli;
/// Module that creates a wallet to get and sweep balances
mod wallet;

use clap::Parser;
use cli::{Cli, Commands};
use thiserror::Error;
use tracing::error;

#[derive(Error, Debug)]
enum FederationUtilsError {
    #[error("Failed to generate key")]
    GenerateKeyFailed,
    #[error("Failed to get balance")]
    GetBalanceFailed,
    #[error("Failed to sweep balance")]
    SweepBalanceFailed,
}

fn inner_main() -> Result<(), FederationUtilsError> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Setup(setup) => {
            println!("Setting up wallet config...");
        }
        cli::Commands::GenerateKey(_) => {
            println!("Generating key...");
        }
        cli::Commands::GetBalance(_) => {
            println!("Getting balance...");
        }
        cli::Commands::SweepBalance(_) => {
            println!("Sweeping balance...");
        }
    }

    Ok(())
}

fn main() {
    if let Err(e) = inner_main() {
        error!("ERROR: {}", e);
    }
}
