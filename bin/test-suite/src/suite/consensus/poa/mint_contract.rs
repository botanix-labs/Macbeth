use crate::mint_contract_abi::MintContract;
use displaydoc::Display as DisplayDoc;
use ethers::{
    contract::ContractError,
    core::{k256::ecdsa::SigningKey, types::Address as EtherAddress},
    middleware::SignerMiddleware,
    providers::{Http, Middleware, Provider, ProviderError},
    signers::{LocalWallet, Signer, Wallet},
    types::{TransactionReceipt, U256},
};
use reth_primitives::BOTANIX_TESTNET;
use secp256k1::SecretKey;
use std::sync::Arc;
use thiserror::Error;
use tracing::info;

/// Contract Error
#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Contract error: `{0}`
    Contract(ContractError<SignerMiddleware<Provider<Http>, Wallet<SigningKey>>>),
    /// Provider error: `{0}`
    Provider(ProviderError),
}

#[derive(Clone, Debug)]
pub struct MintContractInstance {
    mint_contract: MintContract<SignerMiddleware<Provider<Http>, Wallet<SigningKey>>>,
}

impl MintContractInstance {
    pub async fn new(
        rpc_port: u16,
        sender_secret_key: &str,
        mint_contract_address: EtherAddress,
    ) -> Self {
        // Connect to the network
        let provider =
            Provider::<Http>::try_from(&format!("http://127.0.0.1:{}", rpc_port)).unwrap();
        info!("Node URL: {}", &format!("http://127.0.0.1:{}", rpc_port));

        // get chain id
        let chain_id = provider.get_chainid().await.unwrap();
        assert!(U256::from(BOTANIX_TESTNET.chain().id()) == chain_id, "expected same chain id");

        // create a local wallet
        let wallet: LocalWallet =
            sender_secret_key.parse::<LocalWallet>().unwrap().with_chain_id(chain_id.as_u64());

        // connect the wallet to the provider
        let client = SignerMiddleware::new(provider, wallet);

        let mint_contract = MintContract::new(mint_contract_address, Arc::new(client));

        Self { mint_contract }
    }

    pub async fn mint(
        &self,
        destination: ethers::core::types::Address,
        amount: ethers::core::types::U256,
        bitcoin_block_height: u32,
        metadata: ethers::core::types::Bytes,
    ) -> Result<Option<TransactionReceipt>, Error> {
        let tx_receipt = self
            .mint_contract
            .mint(destination, amount, bitcoin_block_height, metadata)
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
    ) -> Result<Option<TransactionReceipt>, Error> {
        let tx_receipt = self
            .mint_contract
            .burn(destination, data)
            .send()
            .await
            .map_err(Error::Contract)?
            .await
            .map_err(Error::Provider)?;
        Ok(tx_receipt)
    }
}
