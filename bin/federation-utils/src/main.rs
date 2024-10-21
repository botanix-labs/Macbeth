//! # Wallet CLI
//!
//! This crate provides a command-line interface for managing a wallet,
//! including setting up the wallet, generating keys, getting the balance,
//! and sweeping the balance.

/// Module that defines the CLI commands
mod cli;
/// Module that creates a wallet to get and sweep balances
mod wallet;

use clap::Parser;
use cli::{Cli, Commands, Setup};
use thiserror::Error;
use tracing::error;
use wallet::WalletConfig;

#[allow(dead_code)]
#[allow(clippy::enum_variant_names)]
#[derive(Error, Debug)]
enum FederationUtilsError {
    #[error("Failed to setup the wallet config: {0}")]
    SetupFailed(#[from] wallet::WalletError),
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
            let Setup {
                chain_id,
                config_path,
                secret_key_destination,
                provider_url,
                receiver_address,
            } = setup;
            let receiver_addres_hash =
                receiver_address.as_ref().map(|addr| addr.parse().expect("Valid address"));
            let config = WalletConfig::new(
                *chain_id,
                config_path.clone(),
                secret_key_destination.clone(),
                provider_url.clone(),
                receiver_addres_hash,
            )?;

            let pretty_config = serde_json::to_string_pretty(&config).expect("Valid JSON");
            println!("Wallet config: {}", pretty_config);
        }
        Commands::GenerateKey(_) => {
            println!("Generating key...");
        }
        Commands::GetBalance(_) => {
            println!("Getting balance...");
        }
        Commands::SweepBalance(_) => {
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
