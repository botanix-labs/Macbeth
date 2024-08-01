use clap::{Parser, Subcommand};
use thiserror::Error;
use tracing::error;

#[derive(Parser)]
#[command(version, about)]
enum App {
    /// Generate the key to sign blocks
    #[command(subcommand)]
    Command(Command),
}

#[derive(Subcommand)]
enum Command {
    /// Generate the key to sign blocks
    #[command()]
    GenerateKey,
    /// Get the wallet balance
    #[command()]
    GetBalance,
    /// Sweep the wallet balance to a destination address
    #[command()]
    SweepBalance,
}

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
    match App::parse() {
        App::Command(cmd) => match cmd {
            Command::GenerateKey => {
                println!("Generating key...");
            }
            Command::GetBalance => {
                println!("Getting balance...");
            }
            Command::SweepBalance => {
                println!("Sweeping balance...");
            }
        },
    }
    Ok(())
}

fn main() {
    if let Err(e) = inner_main() {
        error!("ERROR: {}", e);
    }
}
