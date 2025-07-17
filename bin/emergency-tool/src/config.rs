use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub bitcoind_rpc_url: String,
    pub bitcoind_rpc_user: String,
    pub bitcoind_rpc_pass: String,
    /// Frost participant identifier (our position in federation)
    pub identifier: u16,
    /// The path to the federation configuration file
    pub federation_config_path: PathBuf,
    /// Coordinator identifier (defaults to 0 if not specified)
    pub coordinator: Option<u16>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bitcoind_rpc_url: "http://127.0.0.1:18443".to_string(),
            bitcoind_rpc_user: "regtest".to_string(),
            bitcoind_rpc_pass: "regtest".to_string(),
            identifier: 0,
            federation_config_path: PathBuf::from("federation.toml"),
            coordinator: None,
        }
    }
} 