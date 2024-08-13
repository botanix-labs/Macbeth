use ethers::{
    prelude::*,
    providers::{Http, Provider},
    signers::{Wallet as EthersWallet, WalletError as EthersWalletError},
    types::U256,
};
use serde::{Deserialize, Serialize};
use std::{
    convert::TryFrom,
    fs::File,
    io::{self, Write},
    path::Path,
};
use tracing::error;
use url::ParseError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum WalletError {
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
pub(crate) struct WalletConfig {
    secret_key_output_path: String,
    chain_id: u64,
    receiver_address: Option<Address>,
}

impl WalletConfig {
    pub(crate) fn new(
        config_path: String,
        secret_key_output_path: String,
        chain_id: u64,
        receiver_address: Option<Address>,
    ) -> Result<Self, WalletError> {
        // Append config.json to the path
        let config_path = format!("{}/config.json", config_path);
        let path = Path::new(&config_path);

        // Check if the file already exists
        // TODO: can overwrite with password
        if path.exists() {
            error!("Wallet config file already exists: {:?}", path);
            return Err(WalletError::ConfigFileExists);
        }

        let config = Self { secret_key_output_path, chain_id, receiver_address };

        // serialize the config as JSON
        let config_json = serde_json::to_string(&config)?;

        // Write the config to the path
        let mut file = File::create(path).expect("Failed to create file");
        file.write_all(config_json.as_bytes()).expect("Failed to write to file");

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use ethers::abi::Address;

    use super::WalletConfig;

    #[test]
    fn test_wallet_config_new() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path().to_str().unwrap().to_string();
        let receiver_address = Address::zero();
        let chain_id = 3636;

        let config = WalletConfig::new(
            temp_path.clone(),
            temp_path.clone(),
            chain_id,
            Some(receiver_address),
        );
        assert!(config.is_ok());
        let WalletConfig {
            secret_key_output_path,
            chain_id: result_chain_id,
            receiver_address: result_receiver_address,
        } = config.unwrap();
        assert_eq!(secret_key_output_path, temp_path);
        assert_eq!(result_chain_id, chain_id);
        assert_eq!(result_receiver_address, Some(receiver_address));
    }

    #[test]
    fn test_wallet_config_new_cannot_overwrite_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path().to_str().unwrap().to_string();
        let receiver_address = Address::zero();
        let chain_id = 3636;

        let config = WalletConfig::new(
            temp_path.clone(),
            temp_path.clone(),
            chain_id,
            Some(receiver_address),
        );
        assert!(config.is_ok());

        let config =
            WalletConfig::new(temp_path.clone(), temp_path, chain_id, Some(receiver_address));
        assert!(config.is_err());
        let err = config.unwrap_err();
        assert_eq!(err.to_string(), "Wallet config file already exists");
    }
}
