use crate::config::Config;
use anyhow::Result;
use bitcoin::{Amount, Txid};
use bitcoincore_rpc::{Auth, Client, RpcApi};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Utxo {
    pub txid: Txid,
    pub vout: u32,
    pub amount: Amount,
}

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

    pub async fn get_utxos(&self) -> Result<Vec<Utxo>> {
        let utxos = self.client.list_unspent(None, None, None, None, None)?;
        let utxos = utxos
            .into_iter()
            .map(|u| Utxo {
                txid: u.txid,
                vout: u.vout,
                amount: u.amount,
            })
            .collect();
        Ok(utxos)
    }
} 