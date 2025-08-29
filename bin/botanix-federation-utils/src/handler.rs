use crate::{
    client::{Client, Wallet},
    config::{get_default_config_path, Config},
    errors::WalletError,
};
use base64::Engine;
use ethers::{prelude::*, types::Transaction};
use hex::encode as hex_encode;
use k256::{ecdsa::SigningKey as Secp256k1SigningKey, elliptic_curve::generic_array::GenericArray};
use serde::Deserialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct TendermintKey {
    priv_key: PrivKey,
}

#[derive(Deserialize)]
struct PrivKey {
    #[serde(rename = "type")]
    key_type: String,
    value: String, // This is the base64-encoded private key
}
/// creates botanix-federation-utils config-toml.
pub fn handle_init_config(config_path: Option<&str>) {
    let default_config = Config::default();

    let config_path = config_path.map(PathBuf::from).unwrap_or_else(get_default_config_path);

    let config_file_path = if config_path.to_str().map(|s| s.ends_with('/')).unwrap_or(false) ||
        config_path.extension().is_none()
    {
        config_path.join("config.toml")
    } else {
        config_path
    };

    if let Some(parent_dir) = config_file_path.parent() {
        fs::create_dir_all(parent_dir).expect("Failed to create configuration directory");
    } else {
        panic!("Config file path has no parent directory: {:?}", config_file_path);
    }

    // Serialize the default config and write it to the file
    let config_content = toml::to_string(&default_config).expect("Failed to serialize config");
    let mut file = fs::File::create(&config_file_path).expect("Failed to create config file");
    file.write_all(config_content.as_bytes()).expect("Failed to write to config file");
    println!("Config default values : {:?}", config_content);
    println!("Config file created at: {:?}", config_file_path);
}
/// Fetches the balance for a specified address using the Ethereum provider URL and chain ID,
/// returning a `U256` balance or `WalletError`.
pub async fn handle_get_balance(
    key_path: &str,
    provider_url: &str,
    chain_id: u64,
) -> Result<U256, WalletError> {
    let seceret_path = if !key_path.is_empty() {
        key_path.to_string()
    } else {
        return Err(WalletError::IoError("seceret_key_path not provided".to_string()));
    };

    let private_key =
        if Path::new(&seceret_path).extension().and_then(|ext| ext.to_str()) == Some("json") {
            deserialize_secret_key(&seceret_path)?
        } else {
            fs::read_to_string(&seceret_path)?.trim().to_string()
        };

    if private_key.is_empty() {
        return Err(WalletError::CustomError("Private key cannot be empty".to_string()));
    }
    let client = Client::new(provider_url, chain_id)
        .await
        .map_err(|e| WalletError::IoError(e.to_string()))?;

    let balance = client
        .get_balance(private_key)
        .await
        .map_err(|e| WalletError::BalanceError(e.to_string()))?;

    Ok(balance)
}

/// Sends the balance from one address to another using the Ethereum provider URL and chain ID,
/// returning a transaction hash (`H256`) or `WalletError`.
pub async fn handle_sweep_balance(
    chain_id: u64,
    key_path: &str,
    provider_url: &str,
    receiver_address: &str,
) -> Result<H256, WalletError> {
    let seceret_path = if !key_path.is_empty() {
        key_path.to_string()
    } else {
        return Err(WalletError::IoError("seceret_key_path not provided".to_string()));
    };

    let private_key =
        if Path::new(&seceret_path).extension().and_then(|ext| ext.to_str()) == Some("json") {
            deserialize_secret_key(&seceret_path)?
        } else {
            fs::read_to_string(&seceret_path)?.trim().to_string()
        };

    if private_key.is_empty() {
        return Err(WalletError::CustomError("Private key cannot be empty".to_string()));
    }

    let to_address: H160 = receiver_address
        .parse()
        .map_err(|e| WalletError::InvalidAddress(format!("Failed to parse address: {:?}", e)))?;

    let mut client = Client::new(provider_url, chain_id)
        .await
        .map_err(|e| WalletError::CustomError(format!("Failed to create client: {:?}", e)))?;
    client.chain_id = chain_id;
    println!("handler_peivate_key :: {:?}", private_key);
    match client.sweep_balance(private_key, to_address).await {
        Ok(hash) => Ok(hash),
        Err(e) => Err(WalletError::RpcError(e.to_string())),
    }
}

/// get transactions details
pub async fn handle_get_transaction_info(
    tx_hash: &str,
    provider_url: &str,
    chain_id: u64,
) -> Result<Transaction, WalletError> {
    let hash: TxHash = tx_hash.parse().map_err(|e| {
        WalletError::TransactionNotFound(format!("Failed to parse tx_hash: {:?}", e))
    })?;

    let client = Client::new(provider_url, chain_id)
        .await
        .map_err(|e| WalletError::IoError(e.to_string()))?;

    let tx_info = client
        .get_transaction_info(hash)
        .await
        .map_err(|e| WalletError::BalanceError(e.to_string()))?;

    Ok(tx_info)
}
/// Deseialise Ed25519 or Secp256k1 private key and return string.
fn deserialize_secret_key(seceret_path: &str) -> Result<String, WalletError> {
    let json_data = fs::read_to_string(seceret_path)?;
    let parsed_key: TendermintKey = serde_json::from_str(&json_data)
        .map_err(|e| WalletError::CustomError(format!("Failed to parse JSON: {:?}", e)))?;

    if parsed_key.priv_key.key_type.as_str() == "tendermint/PrivKeyEd25519" {
        return Err(WalletError::CustomError("Invalid key pair type: PrivKeyEd25519".to_string()));
    }
    match parsed_key.priv_key.key_type.as_str() {
        "tendermint/PrivKeySecp256k1" => {
            let engine = base64::engine::general_purpose::STANDARD;
            let priv_key_bytes = engine
                .decode(&parsed_key.priv_key.value)
                .map_err(|e| WalletError::CustomError(format!("Base64 decode error: {:?}", e)))?;

            // Ensure the key is exactly 32 bytes
            let priv_key_array: [u8; 32] = priv_key_bytes.as_slice().try_into().map_err(|_| {
                WalletError::CustomError("Invalid Secp256k1 key length".to_string())
            })?;

            let priv_key_generic = GenericArray::clone_from_slice(&priv_key_array);
            let signing_key = Secp256k1SigningKey::from_bytes(&priv_key_generic).map_err(|e| {
                WalletError::CustomError(format!("Failed to parse Secp256k1 signing key: {:?}", e))
            })?;
            println!("Secp256k1 Signing Key: {:?}", signing_key);

            Ok(hex_encode(priv_key_bytes))
        }
        _ => Err(WalletError::CustomError("Unsupported key type".to_string())),
    }
}
