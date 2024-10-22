//! # Wallet CLI
//!
//! This crate provides a command-line interface for managing a wallet,
//! including setting up the wallet, generating keys, getting the balance,
//! and sweeping the balance.

/// Module that defines the CLI commands
mod cli;
use crate::config::Config;
/// client module
pub mod client;
mod config;
/// error module
pub mod errors;
/// Module handler acts as middleware for command
pub mod handler;

use crate::{config::load_config, errors::WalletError};
use clap::Parser;
use cli::{Cli, Commands};

use handler::{
    handle_get_balance, handle_get_transaction_info, handle_init_config, handle_sweep_balance,
};
use std::path::PathBuf;
async fn inner_main() -> Result<(), WalletError> {
    let cli = Cli::parse();
    let config_path = cli.config_path.as_deref();
    let config = if let Some(path) = config_path {
        load_config(PathBuf::from(path).as_path())
    } else {
        Config::default()
    };
    match &cli.command {
        Commands::Init => {
            println!("initialize config...");
            handle_init_config(config_path);
        }
        Commands::GetBalance(get_balance) => {
            println!("Getting balance...");
            let chain_id = cli.chain_id.unwrap_or(config.chain_id);
            let provider_url = cli.provider_url.as_deref().unwrap_or(&config.provider_url);

            let secret_key_path =
                get_balance.secret_key_path.clone().or(config.secret_path.clone()).ok_or_else(
                    || {
                        WalletError::CustomError(
                            "Secret key path must be provided via CLI or config".to_string(),
                        )
                    },
                )?;
            let bal = handle_get_balance(&secret_key_path, &provider_url, chain_id)
                .await
                .map_err(WalletError::from)?;
            println!("Balance: {:?}", bal);
        }
        Commands::SweepBalance(sweep_balance) => {
            println!("Sweeping balance...");
            let chain_id = cli.chain_id.unwrap_or(config.chain_id);
            let provider_url = cli.provider_url.as_deref().unwrap_or(&config.provider_url);
            let secret_key_path =
                sweep_balance.secret_key_path.clone().or(config.secret_path.clone()).ok_or_else(
                    || {
                        WalletError::CustomError(
                            "Secret key path must be provided via CLI or config".to_string(),
                        )
                    },
                )?;
            let receiver_address = sweep_balance
                .receiver_address
                .clone()
                .or(config.receiver_address.clone())
                .ok_or_else(|| {
                    WalletError::CustomError(
                        "Receiver address must be provided via CLI or config".to_string(),
                    )
                })?;

            let sweep = handle_sweep_balance(
                chain_id,
                &secret_key_path.to_string(),
                &provider_url,
                &receiver_address.to_string(),
            )
            .await
            .map_err(WalletError::from)?;

            println!("Sweep successful: {:?}", sweep);
        }

        Commands::GetTransaction(get_tx) => {
            let chain_id = cli.chain_id.unwrap_or(config.chain_id);
            let provider_url = cli.provider_url.as_deref().unwrap_or(&config.provider_url);
            let tx_hash = get_tx.tx_hash.clone();
            if tx_hash.is_empty() {
                return Err(WalletError::CustomError("Tx hash cannot be an empty".to_string()));
            }
            let tx_info = handle_get_transaction_info(&tx_hash, &provider_url, chain_id)
                .await
                .map_err(WalletError::from)?;

            println!("Transaction info: {:?}", tx_info);
        }
    }

    Ok(())
}
#[tokio::main]
async fn main() {
    if let Err(e) = inner_main().await {
        eprintln!("ERROR: {}", e);
    }
}
