use std::fmt::Debug;

use displaydoc::Display as DisplayDoc;
use ethers::{
    core::types::{Address as EtherAddress, BlockNumber},
    providers::{Http, Middleware, PeerInfo, Provider, ProviderError},
    types::{Block, BlockId, NameOrAddress, Transaction, TransactionReceipt, TxHash, H256, U256},
};
use thiserror::Error;

/// Errors
#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Provider error: `{0}`
    Provider(#[from] ProviderError),
    /// Url parse error: `{0}`
    UrlParse(#[from] url::ParseError),
}

/// Trait defining the interface for Ethereum client operations
#[async_trait::async_trait]
pub trait BotanixEthClientTrait: Send + Sync + Debug {
    /// Get the HTTP provider reference
    fn http_provider(&self) -> &Provider<Http>;

    /// Check if the client is connected to the Ethereum network
    async fn is_connected(&self) -> bool;

    /// Get the nonce for an address (string format)
    async fn nonce(&self, address: &str) -> U256;

    /// Get the balance of some address (Botanix-specific method)
    async fn get_botanix_balance(&self, address: &str) -> Result<U256, Error>;

    /// Get a block by its ID
    async fn get_block_by_id(&self, block_id: BlockId) -> Result<Block<H256>, Error>;

    /// Get transaction receipts for all transactions in a block
    async fn get_tx_receipts(&self, block_id: BlockId) -> Result<Vec<TransactionReceipt>, Error>;

    /// Get balance for an Ethereum address
    async fn get_balance(&self, address: EtherAddress) -> Result<U256, Error>;

    /// Get transaction by hash
    async fn get_tx_by_hash(&self, tx_hash: TxHash) -> Result<Option<Transaction>, Error>;

    /// Get the pending block
    async fn get_pending_block(&self) -> Result<Block<TxHash>, Error>;

    /// Get the latest block hash
    async fn get_latest_block_hash(&self) -> Result<H256, Error>;

    /// Get peer information
    async fn get_peers_counts(&self) -> Result<Vec<PeerInfo>, Error>;

    /// Add a peer by enode URL
    async fn add_peer(&self, enode_url: &str) -> Result<bool, Error>;

    /// Get the latest block by hash
    async fn get_latest_block_by_hash(&self, hash: H256) -> Result<Block<TxHash>, Error>;

    /// Get nonce for an Ethereum address
    async fn get_nonce(&self, address: EtherAddress) -> Result<U256, Error>;

    /// Get the latest block
    async fn get_latest_block(&self) -> Result<Block<TxHash>, Error>;
}

#[derive(Clone, Debug)]
pub struct BotanixEthClient {
    http_client: Provider<Http>,
}

impl BotanixEthClient {
    pub fn new(rpc_port: u16) -> Result<Self, Error> {
        // Connect to the network
        let http_url = format!("http://localhost:{}", rpc_port);
        let http_client = Provider::<Http>::try_from(&http_url).map_err(Error::UrlParse)?;

        Ok(Self { http_client })
    }
}

#[async_trait::async_trait]
impl BotanixEthClientTrait for BotanixEthClient {
    fn http_provider(&self) -> &Provider<Http> {
        self.http_client.provider()
    }

    async fn is_connected(&self) -> bool {
        self.http_client.get_block_number().await.is_ok()
    }

    async fn nonce(&self, address: &str) -> U256 {
        let account =
            NameOrAddress::Address(ethers::types::Address::from_slice(address.as_bytes()));
        self.http_client
            .provider()
            .get_transaction_count(account, Some(BlockId::Number(BlockNumber::Latest)))
            .await
            .expect("nonce to be returned")
    }

    async fn get_botanix_balance(&self, address: &str) -> Result<U256, Error> {
        let sender_account =
            NameOrAddress::Address(ethers::types::Address::from_slice(address.as_bytes()));
        Ok(self.http_client.get_balance(sender_account, None).await.map_err(Error::Provider)?)
    }

    async fn get_block_by_id(&self, block_id: BlockId) -> Result<Block<H256>, Error> {
        Ok(self
            .http_client
            .get_block(block_id)
            .await
            .map_err(Error::Provider)?
            .expect("block exists"))
    }

    async fn get_tx_receipts(&self, block_id: BlockId) -> Result<Vec<TransactionReceipt>, Error> {
        let block = self
            .http_client
            .get_block(block_id)
            .await
            .map_err(Error::Provider)?
            .expect("block exists");

        let transaction_hashes = block.transactions;

        let mut receipts = Vec::new();
        for tx_hash in transaction_hashes {
            let receipt = self
                .http_client
                .get_transaction_receipt(tx_hash)
                .await
                .map_err(Error::Provider)?
                .expect("tx exists");
            receipts.push(receipt);
        }

        Ok(receipts)
    }

    async fn get_balance(&self, address: EtherAddress) -> Result<U256, Error> {
        let sender_account = NameOrAddress::Address(address);
        Ok(self.http_client.get_balance(sender_account, None).await.map_err(Error::Provider)?)
    }

    async fn get_tx_by_hash(&self, tx_hash: TxHash) -> Result<Option<Transaction>, Error> {
        Ok(self.http_client.get_transaction(tx_hash).await.map_err(Error::Provider)?)
    }

    async fn get_pending_block(&self) -> Result<Block<TxHash>, Error> {
        Ok(self
            .http_client
            .get_block(BlockNumber::Pending)
            .await
            .map_err(Error::Provider)?
            .expect("block exists"))
    }

    async fn get_latest_block_hash(&self) -> Result<H256, Error> {
        Ok(self
            .http_client
            .get_block(BlockNumber::Latest)
            .await
            .map_err(Error::Provider)?
            .expect("block exists")
            .hash
            .expect("block hash exists"))
    }

    async fn get_peers_counts(&self) -> Result<Vec<PeerInfo>, Error> {
        Ok(self.http_client.peers().await.map_err(Error::Provider)?)
    }

    async fn add_peer(&self, enode_url: &str) -> Result<bool, Error> {
        Ok(self.http_client.add_peer(enode_url.to_owned()).await.map_err(Error::Provider)?)
    }

    async fn get_latest_block_by_hash(&self, hash: H256) -> Result<Block<TxHash>, Error> {
        Ok(self.http_client.get_block(hash).await.map_err(Error::Provider)?.expect("block exists"))
    }

    async fn get_nonce(&self, address: EtherAddress) -> Result<U256, Error> {
        Ok(self
            .http_client
            .get_transaction_count(address, Some(BlockId::Number(BlockNumber::Latest)))
            .await
            .map_err(Error::Provider)?)
    }

    async fn get_latest_block(&self) -> Result<Block<TxHash>, Error> {
        Ok(self
            .http_client
            .get_block(BlockNumber::Latest)
            .await
            .map_err(Error::Provider)?
            .expect("block exists"))
    }
}

// ======================================================================================= //

/// Mock implementation for testing
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub struct MockBotanixEthClient {
    pub balances: std::collections::HashMap<String, U256>,
    pub nonces: std::collections::HashMap<String, U256>,
    pub blocks: std::collections::HashMap<H256, Block<TxHash>>,
    pub transactions: std::collections::HashMap<TxHash, Transaction>,
    pub transaction_receipts: std::collections::HashMap<TxHash, TransactionReceipt>,
    pub peers: Vec<PeerInfo>,
    pub should_fail: bool, // For testing error scenarios
}

#[cfg(test)]
impl MockBotanixEthClient {
    pub fn new() -> Self {
        Self::default()
    }

    /// Helper method to set up mock data
    pub fn with_balance(mut self, address: &str, balance: U256) -> Self {
        self.balances.insert(address.to_string(), balance);
        self
    }

    /// Helper method to set up mock nonce
    pub fn with_nonce(mut self, address: &str, nonce: U256) -> Self {
        self.nonces.insert(address.to_string(), nonce);
        self
    }

    /// Helper method to simulate failures
    pub fn with_error(mut self) -> Self {
        self.should_fail = true;
        self
    }

    /// Helper method to add a transaction
    pub fn with_transaction(mut self, tx_hash: TxHash, block_number: u64) -> Self {
        let mut tx = Transaction::default();
        tx.hash = tx_hash;
        tx.block_number = Some(block_number.into());
        self.transactions.insert(tx_hash, tx);
        self
    }

    /// Helper method to add a block
    pub fn with_block(mut self, block_number: u64, timestamp: u64) -> Self {
        let mut block = Block::default();
        block.number = Some(block_number.into());
        block.timestamp = timestamp.into();
        block.hash = Some(H256::from_low_u64_be(block_number)); // Simple hash for testing
        self.blocks.insert(block.hash.unwrap(), block);
        self
    }

    /// Helper method to add a recent block (within acceptable timestamp)
    pub fn with_recent_block(self, block_number: u64) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let current_timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        self.with_block(block_number, current_timestamp - 60) // 1 minute ago
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl BotanixEthClientTrait for MockBotanixEthClient {
    fn http_provider(&self) -> &Provider<Http> {
        panic!("http_provider() not supported in mock - consider refactoring the trait")
    }

    async fn is_connected(&self) -> bool {
        if self.should_fail {
            panic!("Mock configured to fail");
        }
        true
    }

    async fn nonce(&self, address: &str) -> U256 {
        if self.should_fail {
            panic!("Mock configured to fail");
        }
        self.nonces.get(address).copied().unwrap_or(U256::zero())
    }

    async fn get_botanix_balance(&self, address: &str) -> Result<U256, Error> {
        if self.should_fail {
            return Err(Error::Provider(ethers::providers::ProviderError::CustomError(
                "Mock error".to_string(),
            )));
        }
        Ok(self.balances.get(address).copied().unwrap_or(U256::zero()))
    }

    async fn get_block_by_id(&self, block_id: BlockId) -> Result<Block<H256>, Error> {
        if self.should_fail {
            return Err(Error::Provider(ethers::providers::ProviderError::CustomError(
                "Mock error".to_string(),
            )));
        }
        // Convert BlockId to find the right block
        let block = match block_id {
            BlockId::Number(ethers::types::BlockNumber::Number(num)) => {
                // Find block by number
                self.blocks.values().find(|b| b.number == Some(num))
            }
            BlockId::Hash(hash) => {
                // Find block by hash
                self.blocks.get(&hash)
            }
            _ => None,
        };

        block.cloned().ok_or_else(|| {
            Error::Provider(ethers::providers::ProviderError::CustomError(
                "Block not found".to_string(),
            ))
        })
    }

    async fn get_tx_receipts(&self, _block_id: BlockId) -> Result<Vec<TransactionReceipt>, Error> {
        if self.should_fail {
            return Err(Error::Provider(ethers::providers::ProviderError::CustomError(
                "Mock error".to_string(),
            )));
        }
        Ok(self.transaction_receipts.values().cloned().collect())
    }

    async fn get_balance(&self, address: EtherAddress) -> Result<U256, Error> {
        if self.should_fail {
            return Err(Error::Provider(ethers::providers::ProviderError::CustomError(
                "Mock error".to_string(),
            )));
        }
        let address_str = format!("{:?}", address);
        Ok(self.balances.get(&address_str).copied().unwrap_or(U256::zero()))
    }

    async fn get_tx_by_hash(&self, tx_hash: TxHash) -> Result<Option<Transaction>, Error> {
        if self.should_fail {
            return Err(Error::Provider(ethers::providers::ProviderError::CustomError(
                "Mock error".to_string(),
            )));
        }
        Ok(self.transactions.get(&tx_hash).cloned())
    }

    async fn get_pending_block(&self) -> Result<Block<TxHash>, Error> {
        if self.should_fail {
            return Err(Error::Provider(ethers::providers::ProviderError::CustomError(
                "Mock error".to_string(),
            )));
        }
        todo!("Implement mock pending block creation")
    }

    async fn get_latest_block_hash(&self) -> Result<H256, Error> {
        if self.should_fail {
            return Err(Error::Provider(ethers::providers::ProviderError::CustomError(
                "Mock error".to_string(),
            )));
        }
        // Return a mock hash
        Ok(H256::zero())
    }

    async fn get_peers_counts(&self) -> Result<Vec<PeerInfo>, Error> {
        if self.should_fail {
            return Err(Error::Provider(ethers::providers::ProviderError::CustomError(
                "Mock error".to_string(),
            )));
        }
        Ok(self.peers.clone())
    }

    async fn add_peer(&self, _enode_url: &str) -> Result<bool, Error> {
        if self.should_fail {
            return Err(Error::Provider(ethers::providers::ProviderError::CustomError(
                "Mock error".to_string(),
            )));
        }
        Ok(true) // Always succeed in mock
    }

    async fn get_latest_block_by_hash(&self, hash: H256) -> Result<Block<TxHash>, Error> {
        if self.should_fail {
            return Err(Error::Provider(ethers::providers::ProviderError::CustomError(
                "Mock error".to_string(),
            )));
        }
        self.blocks.get(&hash).cloned().ok_or_else(|| {
            Error::Provider(ethers::providers::ProviderError::CustomError(
                "Block not found".to_string(),
            ))
        })
    }

    async fn get_nonce(&self, address: EtherAddress) -> Result<U256, Error> {
        if self.should_fail {
            return Err(Error::Provider(ethers::providers::ProviderError::CustomError(
                "Mock error".to_string(),
            )));
        }
        let address_str = format!("{:?}", address);
        Ok(self.nonces.get(&address_str).copied().unwrap_or(U256::zero()))
    }

    async fn get_latest_block(&self) -> Result<Block<TxHash>, Error> {
        if self.should_fail {
            return Err(Error::Provider(ethers::providers::ProviderError::CustomError(
                "Mock error".to_string(),
            )));
        }
        // Return the first block in our mock data, or create a default one
        self.blocks.values().next().cloned().ok_or_else(|| {
            Error::Provider(ethers::providers::ProviderError::CustomError(
                "No blocks available".to_string(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_balance() {
        let mock_client =
            MockBotanixEthClient::new().with_balance("test_address", U256::from(1000));

        let balance = mock_client.get_botanix_balance("test_address").await.unwrap();
        assert_eq!(balance, U256::from(1000));

        let zero_balance = mock_client.get_botanix_balance("unknown_address").await.unwrap();
        assert_eq!(zero_balance, U256::zero());
    }

    #[tokio::test]
    async fn test_mock_error() {
        let mock_client = MockBotanixEthClient::new().with_error();

        let result = mock_client.get_botanix_balance("test_address").await;
        assert!(result.is_err());
    }
}
