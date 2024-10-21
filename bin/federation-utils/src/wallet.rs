use ethers::{
    prelude::*,
    providers::{Http, Provider},
    signers::{Wallet as EthersWallet, WalletError as EthersWalletError},
    types::U256,
};
use serde::{Deserialize, Serialize};
use std::{convert::TryFrom, fs::File, io::Write, path::Path};
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
    #[allow(dead_code)]
    #[error("No chain id was provided")]
    NoChainIdProvided,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct WalletConfig {
    chain_id: u64,
    secret_key_destination: String,
    provider_url: String,
    receiver_address: Option<Address>,
}

const DEFAULT_CONFIG_PATH: &str = "./config.json";
const DEFAULT_SECRET_KEY_PATH: &str = "./secret_key.hex";
const DEFAULT_PROVIDER_URL: &str = "http://localhost:8545";

impl WalletConfig {
    pub(crate) fn new(
        chain_id: u64,
        config_path: Option<String>,
        secret_key_destination: Option<String>,
        provider_url: Option<String>,
        receiver_address: Option<Address>,
    ) -> Result<Self, WalletError> {
        let config_path = config_path
            .map(|path| {
                // WalletProvider::new() will set path to config.json if it wasn't passed
                // TODO: this enforces that the path must end with .json, is this necessary?
                if !path.ends_with(".json") {
                    format!("{}/config.json", path)
                } else {
                    path
                }
            })
            .unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string());
        let path = Path::new(&config_path);

        // Check if the file already exists
        // TODO: can overwrite with password
        if path.exists() {
            error!("Wallet config file already exists: {:?}", path);
            return Err(WalletError::ConfigFileExists);
        }

        let secret_key_destination = secret_key_destination
            .map(|path| format!("{}/secret_key.hex", path))
            .unwrap_or_else(|| DEFAULT_SECRET_KEY_PATH.to_string());

        let provider_url = provider_url.unwrap_or_else(|| DEFAULT_PROVIDER_URL.to_string());

        let config = Self { chain_id, secret_key_destination, provider_url, receiver_address };
        // serialize the config as JSON
        let config_json = serde_json::to_string(&config)?;

        // Write the config to the path
        let mut file = File::create(path).expect("Failed to create file");
        file.write_all(config_json.as_bytes()).expect("Failed to write to file");

        Ok(config)
    }
}

#[allow(dead_code)]
trait Wallet {
    fn get_balance(&self) -> Result<U256, WalletError>;
    fn sweep_balance(&self, to: Address) -> Result<TxHash, WalletError>;
    fn get_config(&self) -> Result<WalletConfig, WalletError>;
}

#[allow(dead_code)]
#[derive(Debug)]
struct WalletProvider {
    wallet: EthersWallet<k256::ecdsa::SigningKey>,
    provider: Provider<Http>,
    config: WalletConfig,
}

impl WalletProvider {
    #[allow(dead_code)]
    // Create a wallet: only chain id needs to be passed if no config exists
    pub(crate) fn new(
        chain_id: Option<u64>,
        config_path: Option<String>,
        secret_key: Option<String>,
        provider_url: Option<String>,
        receiver_address: Option<Address>,
    ) -> Result<Self, WalletError> {
        // use config if exists, otherwise create new config
        let config_path = config_path
            .map(|path| format!("{}/config.json", path))
            .unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string());
        let path = Path::new(&config_path);
        let config = if !path.exists() {
            // chain id must be passed to create config
            let chain_id = chain_id.ok_or(WalletError::NoChainIdProvided)?;

            // create wallet config
            WalletConfig::new(
                chain_id,
                Some(config_path),
                secret_key,
                provider_url,
                receiver_address,
            )?
        } else {
            // load config from file
            let file = File::open(path).expect("Failed to open file");
            let config: WalletConfig = serde_json::from_reader(file).expect("Valid JSON");
            config
        };

        let provider = Provider::<Http>::try_from(config.provider_url.clone())?;
        let sk = std::fs::read_to_string(config.secret_key_destination.clone())
            .expect("Secret key to exist");

        let wallet = match sk.parse::<EthersWallet<k256::ecdsa::SigningKey>>() {
            Ok(wallet) => wallet.with_chain_id(config.chain_id),
            Err(e) => return Err(e.into()),
        };

        Ok(Self { wallet, provider, config })
    }

    // Create a wallet from the config file at the default path if none is provided
    #[allow(dead_code)]
    pub(crate) fn from_config(path: Option<String>) -> Result<Self, WalletError> {
        let path = path.unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string());
        let file = File::open(path).expect("Failed to open file");
        let config: WalletConfig = serde_json::from_reader(file).expect("Valid JSON");

        let provider = Provider::<Http>::try_from(config.provider_url.clone())?;
        let sk = std::fs::read_to_string(config.secret_key_destination.clone())
            .expect("Secret key to exist");
        let wallet = match sk.parse::<EthersWallet<k256::ecdsa::SigningKey>>() {
            Ok(wallet) => wallet.with_chain_id(config.chain_id),
            Err(e) => return Err(e.into()),
        };

        Ok(Self { wallet, provider, config })
    }
}

// TODO: impl Wallet for WalletProvider

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Write, path::Path, result};

    use ethers::abi::Address;

    use crate::wallet::{
        WalletProvider, DEFAULT_CONFIG_PATH, DEFAULT_PROVIDER_URL, DEFAULT_SECRET_KEY_PATH,
    };

    use super::WalletConfig;

    const SK: &str = "52947524bbc14bd90cc86c32b9b7564da2f7f8de343825fed68cd04da4925d29";

    fn remove_config() -> result::Result<(), std::io::Error> {
        std::fs::remove_file(DEFAULT_CONFIG_PATH)
    }

    fn remove_sk() -> result::Result<(), std::io::Error> {
        std::fs::remove_file(DEFAULT_SECRET_KEY_PATH)
    }

    #[test]
    fn test_wallet_config_new_with_only_chain_id() {
        let chain_id = 3636;
        let config = WalletConfig::new(chain_id, None, None, None, None);
        assert!(config.is_ok());
        let WalletConfig {
            chain_id: result_chain_id,
            secret_key_destination,
            provider_url,
            receiver_address: result_receiver_address,
        } = config.unwrap();
        assert_eq!(result_chain_id, chain_id);
        assert_eq!(secret_key_destination, format!("./secret_key.hex"));
        assert_eq!(provider_url, DEFAULT_PROVIDER_URL.to_string());
        assert_eq!(result_receiver_address, None);

        remove_config().ok();
    }

    #[test]
    fn test_wallet_config_new_with_all_arguments() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path().to_str().unwrap().to_string();
        let chain_id = 3636;
        let provider_url = DEFAULT_CONFIG_PATH.to_string();
        let receiver_address = Address::zero();

        let config = WalletConfig::new(
            chain_id,
            Some(temp_path.clone()),
            Some(temp_path.clone()),
            Some(provider_url.clone()),
            Some(receiver_address),
        );
        assert!(config.is_ok());
        let WalletConfig {
            chain_id: result_chain_id,
            secret_key_destination,
            provider_url: result_provider_url,
            receiver_address: result_receiver_address,
        } = config.unwrap();
        assert_eq!(result_chain_id, chain_id);
        assert_eq!(secret_key_destination, format!("{}/secret_key.hex", temp_path));
        assert_eq!(result_provider_url, provider_url);
        assert_eq!(result_receiver_address, Some(receiver_address));

        remove_config().ok();
    }

    #[test]
    fn test_wallet_config_new_cannot_overwrite_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path().to_str().unwrap().to_string();
        let chain_id = 3636;
        let provider_url = DEFAULT_PROVIDER_URL.to_string();
        let receiver_address = Address::zero();

        let config = WalletConfig::new(
            chain_id,
            Some(temp_path.clone()),
            Some(provider_url.clone()),
            Some(temp_path.clone()),
            Some(receiver_address),
        );
        assert!(config.is_ok());

        let config = WalletConfig::new(
            chain_id,
            Some(temp_path.clone()),
            Some(provider_url),
            Some(temp_path),
            Some(receiver_address),
        );
        assert!(config.is_err());
        let err = config.unwrap_err();
        assert_eq!(err.to_string(), "Wallet config file already exists");

        remove_config().ok();
    }

    #[test]
    fn test_wallet_provider_new_with_chain_id_only() {
        // write secret key to default path
        let sk_path = Path::new(DEFAULT_SECRET_KEY_PATH);
        // write secret key to file
        let mut file = File::create(sk_path).expect("Failed to create file");
        file.write_all(SK.as_bytes()).expect("Failed to write to file");

        let chain_id = 3636;
        let result = WalletProvider::new(Some(chain_id), None, None, None, None);
        assert!(result.is_ok());
        let wallet = result.unwrap();
        assert_eq!(wallet.config.chain_id, chain_id);
        assert_eq!(wallet.config.secret_key_destination, format!("./secret_key.hex"));
        assert_eq!(wallet.config.provider_url, DEFAULT_PROVIDER_URL.to_string());
        assert_eq!(wallet.config.receiver_address, None);

        remove_config().ok();
        remove_sk().ok();
    }

    #[test]
    fn test_wallet_provider_new_no_arguments() {
        let result = WalletProvider::new(None, None, None, None, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.to_string(), "No chain id was provided");
    }

    #[test]
    fn test_from_config_with_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path().to_str().unwrap().to_string();
        let chain_id = 3636;
        let provider_url = DEFAULT_PROVIDER_URL.to_string();
        let receiver_address = Address::zero();

        // write secret key to temp path
        let binding = format!("{}/secret_key.hex", temp_path);
        let sk_path = Path::new(binding.as_str());
        // write secret key to file
        let mut file = File::create(sk_path).expect("Failed to create file");
        file.write_all(SK.as_bytes()).expect("Failed to write to file");

        let config = WalletConfig::new(
            chain_id,
            Some(temp_path.clone()),
            Some(temp_path.clone()),
            Some(provider_url.clone()),
            Some(receiver_address),
        );
        assert!(config.is_ok());

        let wallet = WalletProvider::from_config(Some(format!("{}/config.json", temp_path)));
        assert!(wallet.is_ok());
        let wallet = wallet.unwrap();
        assert_eq!(wallet.config.chain_id, chain_id);
        assert_eq!(wallet.config.secret_key_destination, format!("{}/secret_key.hex", temp_path));
        assert_eq!(wallet.config.provider_url, provider_url);
        assert_eq!(wallet.config.receiver_address, Some(receiver_address));

        remove_config().ok();
        remove_sk().ok();
    }

    #[test]
    fn test_from_config_without_path() {
        // write secret key to default path
        let sk_path = Path::new(DEFAULT_SECRET_KEY_PATH);
        // write secret key to file
        let mut file = File::create(sk_path).expect("Failed to create file");
        file.write_all(SK.as_bytes()).expect("Failed to write to file");

        // create wallet at default config path
        let chain_id = 3636;
        let config = WalletConfig::new(chain_id, None, None, None, None);
        assert!(config.is_ok());

        let wallet = WalletProvider::from_config(None);
        assert!(wallet.is_ok());
        let wallet = wallet.unwrap();
        assert_eq!(wallet.config.chain_id, chain_id);
        assert_eq!(wallet.config.secret_key_destination, DEFAULT_SECRET_KEY_PATH.to_string());
        assert_eq!(wallet.config.provider_url, DEFAULT_PROVIDER_URL.to_string());
        assert_eq!(wallet.config.receiver_address, None);

        remove_config().ok();
        remove_sk().ok();
    }
}
