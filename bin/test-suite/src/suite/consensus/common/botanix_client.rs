use crate::{it_info_print, minting::Minting as MintContract};
use anyhow::Context;
use displaydoc::Display as DisplayDoc;
use ethers::{
    contract::ContractError,
    core::{
        k256::ecdsa::SigningKey,
        types::{Address as EtherAddress, Block as EthBlock, BlockNumber},
    },
    middleware::{signer::SignerMiddlewareError, SignerMiddleware},
    providers::{
        ConnectionDetails, Http, Middleware, PeerInfo, Provider, ProviderError, StreamExt, Ws,
    },
    signers::{LocalWallet, Signer, Wallet},
    types::{BlockId, NameOrAddress, TransactionReceipt, TransactionRequest, TxHash, H256, U256},
    utils,
};
use reth_chainspec::BOTANIX_TESTNET;
use reth_primitives::Address;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc::UnboundedSender;

/// Contract Error
#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Contract error: `{0}`
    Contract(ContractError<SignerMiddleware<Provider<Http>, Wallet<SigningKey>>>),
    /// Provider error: `{0}`
    Provider(ProviderError),
    /// Signer middleware error: `{0}`
    SignerMiddleware(SignerMiddlewareError<Provider<Http>, Wallet<SigningKey>>),
}

#[derive(Clone, Debug)]
pub struct BotanixEthClient {
    mint_contract: MintContract<SignerMiddleware<Provider<Http>, Wallet<SigningKey>>>,
    http_client: SignerMiddleware<Provider<Http>, Wallet<SigningKey>>,
    ws_provider: Provider<Ws>,
}

impl BotanixEthClient {
    pub async fn new(
        rpc_port: u16,
        ws_port: u16,
        sender_secret_key: &str,
        mint_contract_address: EtherAddress,
    ) -> anyhow::Result<Self> {
        // Connect to the network
        let http_url = format!("http://127.0.0.1:{}", rpc_port);
        let http_provider =
            Provider::<Http>::try_from(&http_url).context("Failed to create botanix provider")?;
        it_info_print!("Connected to http URL: ", http_url);

        // get chain id
        let chain_id =
            http_provider.get_chainid().await.context("chain id failed to be obtained")?;
        assert!(U256::from(BOTANIX_TESTNET.chain().id()) == chain_id, "expected same chain id");

        // create a local wallet
        let wallet: LocalWallet = sender_secret_key
            .parse::<LocalWallet>()
            .context("failed to parse sender secret key")?
            .with_chain_id(chain_id.as_u64());

        // connect the wallet to the provider
        let http_client = SignerMiddleware::new(http_provider.clone(), wallet);

        // create a ws client
        let ws_url = format!("ws://127.0.0.1:{}", ws_port);
        let ws_conn_details = ConnectionDetails { url: ws_url.clone(), auth: None };
        let ws_provider =
            Provider::<Ws>::connect_with_reconnects(ws_conn_details, 3).await.unwrap();
        it_info_print!("Connecting to WS URL ... ", ws_url);

        // create a mint contract
        let mint_contract = MintContract::new(mint_contract_address, Arc::new(http_client.clone()));

        Ok(Self { mint_contract, http_client, ws_provider })
    }

    pub fn http_client(&self) -> &SignerMiddleware<Provider<Http>, Wallet<SigningKey>> {
        &self.http_client
    }

    pub fn http_provider(&self) -> &Provider<Http> {
        &self.http_client.provider()
    }

    pub fn ws_provider(&self) -> &Provider<Ws> {
        &self.ws_provider
    }

    /// Subscribe to new blocks using WebSocket
    pub async fn subscribe_to_new_blocks(
        &self,
        rx: UnboundedSender<EthBlock<H256>>,
    ) -> anyhow::Result<()> {
        // Subscribe to new blocks
        let mut stream =
            self.ws_provider.subscribe_blocks().await.context("Failed to subscribe to blocks")?;

        // Process the blocks as they are received
        while let Some(block) = stream.next().await {
            let _ = rx.send(block);
        }

        Ok(())
    }

    pub async fn nonce(&self) -> U256 {
        let address = self.http_client.address();
        let nonce = self
            .http_client
            .provider()
            .get_transaction_count(address, Some(BlockId::Number(BlockNumber::Latest)))
            .await
            .expect("nonce to be returned");

        nonce
    }

    pub fn get_sender_address(&self) -> EtherAddress {
        self.http_client.address()
    }

    pub async fn non_confirmed_mint(
        &self,
        destination: EtherAddress,
        amount: ethers::core::types::U256,
        bitcoin_block_height: u32,
        metadata: ethers::core::types::Bytes,
        refund_address: EtherAddress,
        nonce: ethers::core::types::U256,
    ) -> Result<[u8; 32], Error> {
        let gas_price = self.http_client.get_gas_price().await.unwrap();
        let binding = self
            .mint_contract
            .method::<_, H256>(
                "mint",
                (destination, amount, bitcoin_block_height, metadata, refund_address),
            )
            .unwrap()
            .gas_price(gas_price)
            .gas(U256::from(2_000_000))
            .nonce(nonce);
        let prepared_tx = binding.send().await.map_err(Error::Contract)?;

        Ok(prepared_tx.0)
    }

    pub async fn mint(
        &self,
        destination: EtherAddress,
        amount: ethers::core::types::U256,
        bitcoin_block_height: u32,
        metadata: ethers::core::types::Bytes,
        refund_address: EtherAddress,
    ) -> Result<Option<TransactionReceipt>, Error> {
        let gas_price = self.http_client.get_gas_price().await.ok().unwrap_or_default();

        let tx_receipt = self
            .mint_contract
            .mint(destination, amount, bitcoin_block_height, metadata, refund_address)
            .gas_price(gas_price)
            .gas(U256::from(1_000_000))
            .send()
            .await
            .map_err(Error::Contract)?
            .await
            .map_err(Error::Provider)?;
        Ok(tx_receipt)
    }

    pub async fn burn(
        &self,
        destination: ethers::core::types::Bytes,
        data: ethers::core::types::Bytes,
        value: U256,
    ) -> Result<Option<TransactionReceipt>, Error> {
        let gas_price = self.http_client.get_gas_price().await.ok().unwrap_or_default();

        let tx_receipt = self
            .mint_contract
            .burn(destination, data)
            .gas_price(gas_price)
            .value(value)
            .send()
            .await
            .map_err(Error::Contract)?
            .await
            .map_err(Error::Provider)?;
        Ok(tx_receipt)
    }

    /// Get the balance of some address
    /// we leave it as string to allow for different types across ethers and reth primitives
    pub async fn get_botanix_balance(&self, address: Address) -> Result<U256, Error> {
        let sender_account =
            NameOrAddress::Address(ethers::types::Address::from_slice(address.as_slice()));
        let sender_cur_balance = self
            .http_client
            .get_balance(sender_account, None)
            .await
            .map_err(Error::SignerMiddleware)?;
        Ok(sender_cur_balance)
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
            .map_err(Error::SignerMiddleware)?
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
                .map_err(Error::SignerMiddleware)?
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
            .map_err(Error::SignerMiddleware)?;

        Ok(balance)
    }

    pub async fn send_eoa(
        &self,
        receiver_address: EtherAddress,
        amount: u64,
    ) -> Result<Option<TransactionReceipt>, Error> {
        // Eip1559TransactionRequest
        let gas_price = self.http_client.get_gas_price().await.ok().unwrap_or_default();
        let amount = utils::parse_ether(amount.to_string()).expect("amount to be valid");

        // this also knows to estimate the `max_priority_fee_per_gas` but added it manually too
        let tx = TransactionRequest::new()
            .chain_id(BOTANIX_TESTNET.chain().id())
            .to(receiver_address)
            .value(amount)
            .gas_price(gas_price)
            .gas(U256::from(50_000));

        // send the tx with the initialized signer client
        let tx_receipt = self
            .http_client
            .send_transaction(tx, None)
            .await
            .map_err(Error::SignerMiddleware)?
            .await
            .map_err(Error::Provider)?;

        Ok(tx_receipt)
    }

    pub async fn get_pending_block(&self) -> Result<ethers::core::types::Block<TxHash>, Error> {
        let block = self
            .http_client
            .get_block(BlockNumber::Pending)
            .await
            .map_err(Error::SignerMiddleware)?
            .expect("block exists");
        Ok(block)
    }

    pub async fn get_latest_block_hash(&self) -> Result<ethers::core::types::H256, Error> {
        let block_hash = self
            .http_client
            .get_block(BlockNumber::Latest)
            .await
            .map_err(Error::SignerMiddleware)?
            .expect("block exists")
            .hash
            .expect("block hash exists");

        Ok(block_hash)
    }

    pub async fn get_peers_counts(&self) -> Result<Vec<PeerInfo>, Error> {
        let connected_peers = self.http_client.peers().await.map_err(Error::SignerMiddleware)?;

        Ok(connected_peers)
    }

    pub async fn add_peer(&self, enode_url: &str) -> Result<bool, Error> {
        let was_added = self
            .http_client
            .add_peer(enode_url.to_owned())
            .await
            .map_err(Error::SignerMiddleware)?;

        Ok(was_added)
    }

    pub async fn add_trusted_peer(&self, enode_url: &str) -> Result<bool, Error> {
        let was_added = self
            .http_client
            .add_trusted_peer(enode_url.to_owned())
            .await
            .map_err(Error::SignerMiddleware)?;

        Ok(was_added)
    }

    pub async fn remove_peer(&self, enode_url: &str) -> Result<bool, Error> {
        let was_removed = self
            .http_client
            .remove_peer(enode_url.to_owned())
            .await
            .map_err(Error::SignerMiddleware)?;

        Ok(was_removed)
    }

    pub async fn remove_trusted_peer(&self, enode_url: &str) -> Result<bool, Error> {
        let was_removed = self
            .http_client
            .remove_trusted_peer(enode_url.to_owned())
            .await
            .map_err(Error::SignerMiddleware)?;

        Ok(was_removed)
    }

    pub async fn get_latest_block_by_hash(
        &self,
        hash: H256,
    ) -> Result<ethers::core::types::Block<TxHash>, Error> {
        let block = self
            .http_client
            .get_block(hash)
            .await
            .map_err(Error::SignerMiddleware)?
            .expect("block exists");

        Ok(block)
    }

    pub async fn get_nonce(&self, address: EtherAddress) -> Result<U256, Error> {
        let nonce = self
            .http_client
            .get_transaction_count(address, Some(BlockId::Number(BlockNumber::Latest)))
            .await
            .map_err(Error::SignerMiddleware)
            .expect("nonce exists");

        Ok(nonce)
    }

    pub async fn get_latest_block(&self) -> Result<ethers::core::types::Block<TxHash>, Error> {
        let latest_block = self
            .http_client
            .get_block(BlockNumber::Latest)
            .await
            .map_err(Error::SignerMiddleware)?
            .expect("block exists");

        Ok(latest_block)
    }
}
