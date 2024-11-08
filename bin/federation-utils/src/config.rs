/// Config struct to be written to config.toml
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct Config {
    pub chain_id: u64,
    pub provider_url: String,
    pub receiver_address: Option<String>,
    pub secret_path: Option<String>,
}

/// Loads the configuration from the provided path or the default path
pub(crate) fn load_config(config_path: &Path) -> Config {
    if config_path.exists() {
        let config_content = fs::read_to_string(config_path).expect("Failed to read config file");
        let loaded_config: Config =
            toml::from_str(&config_content).expect("Failed to parse config file");

        loaded_config
    } else {
        Config::default()
    }
}
//create config file insdie the home directory.
pub(crate) fn get_default_config_path() -> PathBuf {
    dirs::home_dir().expect("Failed to get home directory").join("fed-utils").join("config.toml")
}

impl Default for Config {
    fn default() -> Self {
        Self {
            chain_id: 3636,
            provider_url: "http://localhost:8545".to_string(),
            receiver_address: Some("<enter-receiver-address>".to_string()),
            secret_path: Some("<path-to-secret>".to_string()),
        }
    }
}
