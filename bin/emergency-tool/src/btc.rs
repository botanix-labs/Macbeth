use crate::config::Config;
use anyhow::Result;
use bitcoincore_rpc::{Auth, Client, RpcApi};

pub struct BtcClient {
    client: Client,
}

impl BtcClient {
    pub fn new(config: &Config) -> Result<Self> {
        let rpc = Client::new(
            &config.bitcoind_rpc_url,
            Auth::UserPass(
                config.bitcoind_rpc_user.clone(),
                config.bitcoind_rpc_pass.clone(),
            ),
        )?;
        Ok(Self { client: rpc })
    }

    pub async fn get_utxos(&self) -> Result<Vec<bitcoincore_rpc::json::ListUnspentResultEntry>> {
        let utxos = self.client.list_unspent(None, None, None, None, None)?;
        Ok(utxos)
    }
} 