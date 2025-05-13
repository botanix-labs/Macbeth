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

#[derive(Clone, Debug)]
pub struct BotanixEthClient {
    http_client: Provider<Http>,
}

impl BotanixEthClient {
    pub fn new(rpc_port: u16) -> Result<Self, Error> {
        // Connect to the network
        let http_url = format!("http://127.0.0.1:{}", rpc_port);
        let http_client = Provider::<Http>::try_from(&http_url).map_err(|e| Error::UrlParse(e))?;

        Ok(Self { http_client })
    }

    pub fn http_provider(&self) -> &Provider<Http> {
        &self.http_client.provider()
    }

    pub async fn nonce(&self, address: &str) -> U256 {
        let account =
            NameOrAddress::Address(ethers::types::Address::from_slice(address.as_bytes()));
        let nonce = self
            .http_client
            .provider()
            .get_transaction_count(account, Some(BlockId::Number(BlockNumber::Latest)))
            .await
            .expect("nonce to be returned");
        nonce
    }

    /// Get the balance of some address
    /// we leave it as string to allow for different types across ethers and reth primitives
    pub async fn get_botanix_balance(&self, address: &str) -> Result<U256, Error> {
        let sender_account =
            NameOrAddress::Address(ethers::types::Address::from_slice(address.as_bytes()));
        let sender_cur_balance = self
            .http_client
            .get_balance(sender_account, None)
            .await
            .map_err(|e| Error::Provider(e))?;
        Ok(sender_cur_balance)
    }

    pub async fn get_block_by_id(&self, block_id: BlockId) -> Result<Block<H256>, Error> {
        // Get the block by ID (either hash or number)
        let block = self
            .http_client
            .get_block(block_id)
            .await
            .map_err(|e| Error::Provider(e))?
            .expect("block exists");
        Ok(block)
    }

    pub async fn get_tx_receipts(
        &self,
        block_id: BlockId,
    ) -> Result<Vec<TransactionReceipt>, Error> {
        // Get the block by ID (either hash or number)
        let block = self
            .http_client
            .get_block(block_id)
            .await
            .map_err(|e| Error::Provider(e))?
            .expect("block exists");

        // Get all transaction hashes from the block
        let transaction_hashes = block.transactions;

        // Fetch the transaction receipts for all transactions in the block
        let mut receipts = Vec::new();
        for tx_hash in transaction_hashes {
            let receipt = self
                .http_client
                .get_transaction_receipt(tx_hash)
                .await
                .map_err(|e| Error::Provider(e))?
                .expect("tx exists");
            receipts.push(receipt);
        }

        Ok(receipts)
    }

    pub async fn get_balance(&self, address: ethers::core::types::Address) -> Result<U256, Error> {
        let sender_account = NameOrAddress::Address(address);
        let balance = self
            .http_client
            .get_balance(sender_account, None)
            .await
            .map_err(|e| Error::Provider(e))?;

        Ok(balance)
    }

    pub async fn get_tx_by_hash(&self, tx_hash: TxHash) -> Result<Option<Transaction>, Error> {
        let tx = self.http_client.get_transaction(tx_hash).await.map_err(|e| Error::Provider(e))?;
        Ok(tx)
    }

    pub async fn get_pending_block(&self) -> Result<ethers::core::types::Block<TxHash>, Error> {
        let block = self
            .http_client
            .get_block(BlockNumber::Pending)
            .await
            .map_err(|e| Error::Provider(e))?
            .expect("block exists");
        Ok(block)
    }

    pub async fn get_latest_block_hash(&self) -> Result<ethers::core::types::H256, Error> {
        let block_hash = self
            .http_client
            .get_block(BlockNumber::Latest)
            .await
            .map_err(|e| Error::Provider(e))?
            .expect("block exists")
            .hash
            .expect("block hash exists");

        Ok(block_hash)
    }

    pub async fn get_peers_counts(&self) -> Result<Vec<PeerInfo>, Error> {
        let connected_peers = self.http_client.peers().await.map_err(|e| Error::Provider(e))?;
        Ok(connected_peers)
    }

    pub async fn add_peer(&self, enode_url: &str) -> Result<bool, Error> {
        let was_added = self
            .http_client
            .add_peer(enode_url.to_owned())
            .await
            .map_err(|e| Error::Provider(e))?;

        Ok(was_added)
    }

    pub async fn get_latest_block_by_hash(
        &self,
        hash: H256,
    ) -> Result<ethers::core::types::Block<TxHash>, Error> {
        let block = self
            .http_client
            .get_block(hash)
            .await
            .map_err(|e| Error::Provider(e))?
            .expect("block exists");

        Ok(block)
    }

    pub async fn get_nonce(&self, address: EtherAddress) -> Result<U256, Error> {
        let nonce = self
            .http_client
            .get_transaction_count(address, Some(BlockId::Number(BlockNumber::Latest)))
            .await
            .map_err(|e| Error::Provider(e))?;

        Ok(nonce)
    }

    pub async fn get_latest_block(&self) -> Result<ethers::core::types::Block<TxHash>, Error> {
        let latest_block = self
            .http_client
            .get_block(BlockNumber::Latest)
            .await
            .map_err(|e| Error::Provider(e))?
            .expect("block exists");

        Ok(latest_block)
    }
}
