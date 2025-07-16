use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub bitcoind_rpc_url: String,
    pub bitcoind_rpc_user: String,
    pub bitcoind_rpc_pass: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bitcoind_rpc_url: "http://127.0.0.1:18443".to_string(),
            bitcoind_rpc_user: "regtest".to_string(),
            bitcoind_rpc_pass: "regtest".to_string(),
        }
    }
} 