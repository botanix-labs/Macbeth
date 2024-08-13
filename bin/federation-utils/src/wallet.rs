use ethers::prelude::*;
use ethers::providers::{Http, Provider};
use ethers::signers::{Wallet as EthersWallet, WalletError as EthersWalletError};
use ethers::types::U256;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use tracing::error;
use url::ParseError;

#[derive(Debug, thiserror::Error)]
enum WalletError {
    #[error("Failed to create the provider: {0}")]
    ProviderCreationFailed(#[from] ParseError),
    #[error("Failed to create the wallet: {0}")]
    WalletCreationFailed(#[from] EthersWalletError),
    #[error("Failed to serialize the wallet config: {0}")]
    ConfigSerializationFailed(#[from] serde_json::Error),
    #[error("Wallet config file already exists")]
    ConfigFileExists,
}

#[derive(Debug, Serialize, Deserialize)]
struct WalletConfig {
    secret_key_output_path: String,
    chain_id: u64,
    receiver_address: Option<Address>,
}

impl WalletConfig {
    fn new(
        config_path: String,
        secret_key_output_path: String,
        chain_id: u64,
        receiver_address: Option<Address>,
    ) -> Result<(), WalletError> {
        let path = Path::new(&config_path);

        // Check if the file already exists
        // TODO: can overwrite with password
        if path.exists() {
            error!("Wallet config file already exists: {}", config_path);
            return Err(WalletError::ConfigFileExists);
        }

        let config = Self { secret_key_output_path, chain_id, receiver_address };

        // serialize the config as JSON
        let config_json = serde_json::to_string(&config)?;

        // Write the config to the path
        let mut file = File::create(&path).expect("Failed to create file");
        file.write_all(config_json.as_bytes()).expect("Failed to write to file");

        // write to stdout
        io::stdout().flush().expect("To flush to stdout");

        Ok(())
    }
}
