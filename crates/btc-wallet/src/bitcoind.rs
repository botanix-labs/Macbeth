use bitcoincore_rpc::{
    json::{EstimateMode, EstimateSmartFeeResult, GetBlockResult},
    Auth, Client, RpcApi,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum BitcoindError {
    #[error("Client initialization failed")]
    ClientInitFailed(bitcoincore_rpc::Error),
    #[error("Block Header retrieval failed")]
    BlockHeaderRetrievalFailed(bitcoincore_rpc::Error),
    #[error("Block Tip retrieval failed")]
    BlockTipRetrievalFailed(bitcoincore_rpc::Error),
    #[error("Empty block tip")]
    EmptyBlockTip,
    #[error("Block hash retrieval failed")]
    BlockHashRetrievalFailed(bitcoincore_rpc::Error),
    #[error("Tx broadcast failed")]
    TransactionBroadcastFailed(bitcoincore_rpc::Error),
    #[error("Block index failed")]
    BlockIndexStatusFailed(bitcoincore_rpc::Error),
    #[error("Best block hash retrieval failed")]
    BestBlockHashRetrievalFailed(bitcoincore_rpc::Error),
    #[error("Block info retrieval failed")]
    BlockInfoRetrievalFailed(bitcoincore_rpc::Error),
    #[error("Smart estimate fee retrieval failed")]
    EstimateSmartFeeFailed(bitcoincore_rpc::Error),
}

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct BitcoindConfig {
    url: Url,
    username: String,
    password: String,
}

impl BitcoindConfig {
    pub fn new(url: Url, username: String, password: String) -> Self {
        Self { url, username, password }
    }
}
#[derive(Debug)]
pub struct BitcoindClient {
    rpc: Client,
}

impl BitcoindClient {
    pub fn new(config: BitcoindConfig) -> Result<Self, BitcoindError> {
        let BitcoindConfig { url, username, password } = config;
        let creds = Auth::UserPass(username, password);
        let rpc = Client::new(url.to_string().as_str(), creds)
            .map_err(BitcoindError::ClientInitFailed)?;
        Ok(BitcoindClient { rpc })
    }

    pub fn get_rpc_client(&self) -> &Client {
        &self.rpc
    }

    pub async fn get_best_block_hash(&self) -> Result<bitcoin::BlockHash, BitcoindError> {
        let best_block_hash =
            self.rpc.get_best_block_hash().map_err(BitcoindError::BestBlockHashRetrievalFailed)?;
        Ok(best_block_hash)
    }

    pub async fn get_block_header(
        &self,
        block_hash: bitcoin::BlockHash,
    ) -> Result<bitcoin::blockdata::block::Header, BitcoindError> {
        let header = self
            .rpc
            .get_block_header(&block_hash)
            .map_err(BitcoindError::BlockHeaderRetrievalFailed)?;
        Ok(header)
    }

    pub async fn is_synced(&self) -> Result<bool, BitcoindError> {
        let index_data =
            self.rpc.get_index_info().map_err(BitcoindError::BlockIndexStatusFailed)?;
        match index_data.txindex {
            Some(txindex) => Ok(txindex.synced),
            _ => Ok(false),
        }
    }

    pub async fn get_block_hash(&self, height: u64) -> Result<bitcoin::BlockHash, BitcoindError> {
        let block_hash =
            self.rpc.get_block_hash(height).map_err(BitcoindError::BlockHeaderRetrievalFailed)?;
        Ok(block_hash)
    }

    pub async fn get_block_info(
        &self,
        block_hash: &bitcoin::BlockHash,
    ) -> Result<GetBlockResult, BitcoindError> {
        let block =
            self.rpc.get_block_info(block_hash).map_err(BitcoindError::BlockInfoRetrievalFailed)?;
        Ok(block)
    }

    pub async fn get_tip(&self) -> Result<u64, BitcoindError> {
        let tip = self.rpc.get_block_count().map_err(BitcoindError::BlockTipRetrievalFailed)?;

        Ok(tip)
    }

    pub async fn get_txids(
        &self,
        block_hash: bitcoin::BlockHash,
    ) -> Result<Vec<bitcoin::Txid>, BitcoindError> {
        let block = self
            .rpc
            .get_block_info(&block_hash)
            .map_err(BitcoindError::BlockHeaderRetrievalFailed)?;
        Ok(block.tx)
    }

    pub async fn broadcast_tx(&self, raw_tx: &String) -> Result<bitcoin::Txid, BitcoindError> {
        let tx_id = self
            .rpc
            .send_raw_transaction(raw_tx.to_owned())
            .map_err(BitcoindError::TransactionBroadcastFailed)?;
        Ok(tx_id)
    }

    pub async fn wait_until_synced(&self) {
        loop {
            match self.is_synced().await {
                Ok(is_synced) => {
                    if !is_synced {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                    break;
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            }
        }
    }

    pub async fn get_estimate_smart_fee(&self) -> Result<EstimateSmartFeeResult, BitcoindError> {
        let fee_res = self
            .rpc
            .estimate_smart_fee(1, Some(EstimateMode::Conservative))
            .map_err(BitcoindError::EstimateSmartFeeFailed);

        fee_res
    }
}

mod tests {

    #[tokio::test]
    async fn test_basic_client() {
        use super::*;

        let client = BitcoindClient::new(BitcoindConfig::new(
            "http://127.0.0.1:38332".parse::<Url>().unwrap(),
            "usr".to_owned(),
            "pwd".to_owned(),
        ))
        .unwrap();

        let tip = client.get_tip().await;
        match tip {
            Ok(tip) => {
                assert!(tip > 0);
            }
            Err(e) => {
                panic!("Got error {:?}", e);
            }
        }
    }
}
