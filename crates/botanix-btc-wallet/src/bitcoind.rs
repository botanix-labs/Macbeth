use async_trait::async_trait;
pub use bitcoincore_rpc::{
    json::{EstimateMode, EstimateSmartFeeResult, GetBlockHeaderResult},
    jsonrpc, Auth, Client, Error as JsonRPCError, RpcApi,
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
    #[error("Blockchain index failed")]
    BlockchainInfoFailed(bitcoincore_rpc::Error),
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

impl Default for BitcoindConfig {
    fn default() -> Self {
        Self {
            url: Url::parse("http://localhost:18843").unwrap(),
            username: "foo".to_string(),
            password: "bar".to_string(),
        }
    }
}

impl BitcoindConfig {
    pub fn url(&self) -> &Url {
        &self.url
    }
    pub fn username(&self) -> &str {
        &self.username
    }
    pub fn password(&self) -> &str {
        &self.password
    }
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

pub trait BitcoindFactory: Clone + Send + Sync {
    fn new(config: BitcoindConfig) -> Self;
    fn build_and_connect(&self) -> Result<impl RpcApiExt, JsonRPCError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BitcoindClientFactory {
    config: BitcoindConfig,
}

#[allow(async_fn_in_trait)]
#[async_trait]
pub trait RpcApiExt: RpcApi + Send + Sync + 'static {
    async fn is_synced(&self) -> Result<bool, BitcoindError>;
    async fn wait_until_synced(&self);
}

#[async_trait]
impl RpcApiExt for Client {
    async fn is_synced(&self) -> Result<bool, BitcoindError> {
        #[derive(Deserialize)]
        struct Res {
            initialblockdownload: bool,
        }

        match self
            .call::<Res>("getblockchaininfo", &[])
            .map_err(BitcoindError::BlockchainInfoFailed)
        {
            Ok(blockchain_info_result) => Ok(!blockchain_info_result.initialblockdownload),
            Err(err) => {
                tracing::error!("error getting get_blockchain_info(): {:?}", err);
                Ok(false)
            }
        }
    }

    async fn wait_until_synced(&self) {
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
}

impl BitcoindFactory for BitcoindClientFactory {
    fn new(config: BitcoindConfig) -> Self {
        Self { config }
    }

    fn build_and_connect(&self) -> Result<impl RpcApiExt, JsonRPCError> {
        let BitcoindConfig { url, username, password } = &self.config;
        let creds = Auth::UserPass(username.clone(), password.clone());
        let rpc = Client::new(url.to_string().as_str(), creds)?;
        Ok(rpc)
    }
}

// TODO(armins) we dont really need this. We can just use BitcoindClientFactory directly
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

    pub fn get_best_block_hash(&self) -> Result<bitcoin::BlockHash, BitcoindError> {
        let best_block_hash =
            self.rpc.get_best_block_hash().map_err(BitcoindError::BestBlockHashRetrievalFailed)?;
        Ok(best_block_hash)
    }

    pub fn get_block_header(
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
        #[derive(Deserialize)]
        struct Res {
            initialblockdownload: bool,
        }

        match self
            .rpc
            .call::<Res>("getblockchaininfo", &[])
            .map_err(BitcoindError::BlockchainInfoFailed)
        {
            Ok(blockchain_info_result) => Ok(!blockchain_info_result.initialblockdownload),
            Err(err) => {
                // TODO (armins) use logger library
                println!("error getting get_blockchain_info(): {:?}", err);
                Ok(false)
            }
        }
    }

    pub fn get_block_hash(&self, height: u64) -> Result<bitcoin::BlockHash, BitcoindError> {
        let block_hash =
            self.rpc.get_block_hash(height).map_err(BitcoindError::BlockHeaderRetrievalFailed)?;
        Ok(block_hash)
    }

    pub fn get_block_info(
        &self,
        block_hash: &bitcoin::BlockHash,
    ) -> Result<GetBlockHeaderResult, BitcoindError> {
        let block = self
            .rpc
            .get_block_header_info(block_hash)
            .map_err(BitcoindError::BlockInfoRetrievalFailed)?;
        Ok(block)
    }

    pub fn get_txids(
        &self,
        block_hash: bitcoin::BlockHash,
    ) -> Result<Vec<bitcoin::Txid>, BitcoindError> {
        let block = self
            .rpc
            .get_block_info(&block_hash)
            .map_err(BitcoindError::BlockHeaderRetrievalFailed)?;
        Ok(block.tx)
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

    pub fn get_estimate_smart_fee(&self) -> Result<EstimateSmartFeeResult, BitcoindError> {
        self.rpc
            .estimate_smart_fee(1, Some(EstimateMode::Conservative))
            .map_err(BitcoindError::EstimateSmartFeeFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{block::Version, hashes::Hash, Amount, CompactTarget};
    use bitcoincore_rpc::{
        bitcoin::{BlockHash, Txid},
        jsonrpc::serde_json,
    };
    use std::str::FromStr;

    struct MockBitcoindClient {
        // Pre-defined responses for each method we want to test
        best_block_hash: BlockHash,
        block_header: bitcoin::blockdata::block::Header,
        block_hash: BlockHash,
        block_header_info: GetBlockHeaderResult,
        block_info: bitcoincore_rpc::json::GetBlockResult,
        fee_result: EstimateSmartFeeResult,
        is_synced: bool,
    }

    impl MockBitcoindClient {
        fn new(is_synced: bool) -> Self {
            // Create a default block hash used in several responses
            let block_hash = BlockHash::from_str(
                "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f",
            )
            .unwrap();

            // Create a default header for block header responses
            let header = bitcoin::blockdata::block::Header {
                version: Version::ONE,
                prev_blockhash: BlockHash::all_zeros(),
                merkle_root: bitcoin::hash_types::TxMerkleNode::all_zeros(),
                time: 1231006505,
                bits: CompactTarget::from_consensus(1),
                nonce: 2083236893,
            };

            // Create a default txid used in block info
            let txid =
                Txid::from_str("4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b")
                    .unwrap();

            // Create a block header info response
            let header_result = GetBlockHeaderResult {
                hash: block_hash,
                confirmations: 1,
                height: 0,
                version: Version::ONE,
                version_hex: Some(Vec::new()),
                merkle_root: "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
                    .parse()
                    .unwrap(),
                time: 1231006505,
                median_time: Some(1231006505),
                nonce: 2083236893,
                bits: "1d00ffff".to_string(),
                difficulty: 1.0.into(),
                chainwork: Vec::new(),
                n_tx: 1,
                previous_block_hash: None,
                next_block_hash: None,
            };

            // Create a block info response
            let block_result = bitcoincore_rpc::json::GetBlockResult {
                hash: block_hash,
                confirmations: 1,
                size: 285,
                strippedsize: Some(285),
                weight: 1140,
                height: 0,
                version: 1,
                version_hex: Some(Vec::new()),
                merkleroot: "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
                    .parse()
                    .unwrap(),
                tx: vec![txid],
                time: 1231006505,
                mediantime: Some(1231006505),
                nonce: 2083236893,
                bits: "1d00ffff".to_string(),
                difficulty: 1.0.into(),
                chainwork: Vec::new(),
                n_tx: 1,
                previousblockhash: None,
                nextblockhash: Some(
                    "00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048"
                        .parse()
                        .unwrap(),
                ),
            };

            // Create a fee estimate response
            let fee_result =
                EstimateSmartFeeResult { fee_rate: Some(Amount::ONE_BTC), blocks: 6, errors: None };

            Self {
                best_block_hash: block_hash,
                block_header: header,
                block_hash,
                block_header_info: header_result,
                block_info: block_result,
                fee_result,
                is_synced,
            }
        }

        fn get_best_block_hash(&self) -> Result<bitcoin::BlockHash, BitcoindError> {
            Ok(self.best_block_hash)
        }

        fn get_block_header(
            &self,
            _block_hash: bitcoin::BlockHash,
        ) -> Result<bitcoin::blockdata::block::Header, BitcoindError> {
            Ok(self.block_header)
        }

        fn get_block_hash(&self, _height: u64) -> Result<bitcoin::BlockHash, BitcoindError> {
            Ok(self.block_hash)
        }

        fn get_block_info(
            &self,
            _block_hash: &bitcoin::BlockHash,
        ) -> Result<GetBlockHeaderResult, BitcoindError> {
            Ok(self.block_header_info.clone())
        }

        fn get_txids(
            &self,
            _block_hash: bitcoin::BlockHash,
        ) -> Result<Vec<bitcoin::Txid>, BitcoindError> {
            Ok(self.block_info.tx.clone())
        }

        fn get_estimate_smart_fee(&self) -> Result<EstimateSmartFeeResult, BitcoindError> {
            Ok(self.fee_result.clone())
        }

        async fn is_synced(&self) -> Result<bool, BitcoindError> {
            Ok(self.is_synced)
        }
    }

    #[test]
    fn test_bitcoind_config_default() {
        let config = BitcoindConfig::default();

        assert_eq!(config.url().as_str(), "http://localhost:18843/");
        assert_eq!(config.username(), "foo");
        assert_eq!(config.password(), "bar");
    }

    #[test]
    fn test_bitcoind_config_new() {
        let url = Url::parse("http://127.0.0.1:8332").unwrap();
        let username = "testuser".to_string();
        let password = "testpass".to_string();

        let config = BitcoindConfig::new(url.clone(), username.clone(), password.clone());

        assert_eq!(config.url(), &url);
        assert_eq!(config.username(), &username);
        assert_eq!(config.password(), &password);
    }

    #[test]
    fn test_bitcoind_config_getters() {
        let url = Url::parse("http://btc.example.com:8332").unwrap();
        let username = "alice".to_string();
        let password = "secret123".to_string();

        let config = BitcoindConfig::new(url.clone(), username.clone(), password.clone());

        // Test the getters
        assert_eq!(config.url(), &url);
        assert_eq!(config.username(), &username);
        assert_eq!(config.password(), &password);
    }

    #[test]
    fn test_bitcoind_config_serialization() {
        let url = Url::parse("http://192.168.1.10:8332").unwrap();
        let username = "bob".to_string();
        let password = "p@ssw0rd".to_string();

        let config = BitcoindConfig::new(url, username, password);

        // Serialize to JSON
        let serialized = serde_json::to_string(&config).unwrap();

        // Deserialize back
        let deserialized: BitcoindConfig = serde_json::from_str(&serialized).unwrap();

        // Check equality
        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_bitcoind_config_clone() {
        let config = BitcoindConfig::default();
        let cloned_config = config.clone();

        assert_eq!(config, cloned_config);
    }

    #[test]
    fn test_bitcoind_client_factory_new() {
        let config = BitcoindConfig::default();
        let factory = BitcoindClientFactory::new(config.clone());

        assert_eq!(factory.config, config);
    }

    #[test]
    fn test_bitcoind_client_get_best_block_hash() {
        let client = MockBitcoindClient::new(true);

        let result = client.get_best_block_hash();
        assert!(result.is_ok());

        let hash = result.unwrap();
        assert_eq!(
            hash.to_string(),
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
        );
    }

    #[test]
    fn test_bitcoind_client_get_block_header() {
        let client = MockBitcoindClient::new(true);
        let block_hash =
            BlockHash::from_str("000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f")
                .unwrap();

        let result = client.get_block_header(block_hash);
        assert!(result.is_ok());

        let header = result.unwrap();
        assert_eq!(header.time, 1231006505);
    }

    #[test]
    fn test_bitcoind_client_get_block_info() {
        let client = MockBitcoindClient::new(true);
        let block_hash =
            BlockHash::from_str("000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f")
                .unwrap();

        let result = client.get_block_info(&block_hash);
        assert!(result.is_ok());

        let info = result.unwrap();
        assert_eq!(info.height, 0);
    }

    #[test]
    fn test_bitcoind_client_get_txids() {
        let client = MockBitcoindClient::new(true);
        let block_hash =
            BlockHash::from_str("000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f")
                .unwrap();

        let result = client.get_txids(block_hash);
        assert!(result.is_ok());

        let txids = result.unwrap();
        assert_eq!(txids.len(), 1);
        assert_eq!(
            txids[0].to_string(),
            "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
        );
    }

    #[test]
    fn test_bitcoind_client_get_estimate_smart_fee() {
        let client = MockBitcoindClient::new(true);

        let result = client.get_estimate_smart_fee();
        assert!(result.is_ok());

        let fee = result.unwrap();
        assert_eq!(fee.blocks, 6);
        assert!(fee.fee_rate.is_some());
    }

    #[tokio::test]
    async fn test_bitcoind_client_is_synced_true() {
        let client = MockBitcoindClient::new(true);

        let result = client.is_synced().await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_bitcoind_client_is_synced_false() {
        let client = MockBitcoindClient::new(false);

        let result = client.is_synced().await;
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }
}
