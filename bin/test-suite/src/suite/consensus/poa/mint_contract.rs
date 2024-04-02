use crate::{it_info_print, mint_contract_abi::MintContract};
use displaydoc::Display as DisplayDoc;
use ethers::{
    contract::ContractError,
    core::{k256::ecdsa::SigningKey, types::Address as EtherAddress},
    etherscan::account,
    middleware::{signer::SignerMiddlewareError, SignerMiddleware},
    providers::{Http, Middleware, Provider, ProviderError},
    signers::{LocalWallet, Signer, Wallet},
    types::{
        Eip1559TransactionRequest, NameOrAddress, TransactionReceipt, TransactionRequest, U256,
    },
    utils,
};
use reth_primitives::BOTANIX_TESTNET;
use std::{str::FromStr, sync::Arc};
use thiserror::Error;

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
pub struct MintContractInstance {
    mint_contract: MintContract<SignerMiddleware<Provider<Http>, Wallet<SigningKey>>>,
    client: SignerMiddleware<Provider<Http>, Wallet<SigningKey>>,
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
        it_info_print!("Node URL: ", &format!("http://127.0.0.1:{}", rpc_port));

        // get chain id
        let chain_id = provider.get_chainid().await.unwrap();
        assert!(U256::from(BOTANIX_TESTNET.chain().id()) == chain_id, "expected same chain id");

        // create a local wallet
        let wallet: LocalWallet =
            sender_secret_key.parse::<LocalWallet>().unwrap().with_chain_id(chain_id.as_u64());

        // connect the wallet to the provider
        let client = SignerMiddleware::new(provider.clone(), wallet);
        let client2 = client.clone();

        let mint_contract = MintContract::new(mint_contract_address, Arc::new(client));

        Self { mint_contract, client: client2 }
    }

    pub async fn mint(
        &self,
        destination: ethers::core::types::Address,
        amount: ethers::core::types::U256,
        bitcoin_block_height: u32,
        metadata: ethers::core::types::Bytes,
        refund_address: ethers::core::types::Address,
    ) -> Result<Option<TransactionReceipt>, Error> {
        let gas_price = self.client.get_gas_price().await.ok().unwrap_or_default();

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
        let gas_price = self.client.get_gas_price().await.ok().unwrap_or_default();

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

    pub async fn send_eoa(
        &self,
        receiver_address: ethers::core::types::Address,
        amount: u64,
    ) -> Result<Option<TransactionReceipt>, Error> {
        // Eip1559TransactionRequest
        let gas_price = self.client.get_gas_price().await.ok().unwrap_or_default();
        let amount = utils::parse_ether(amount.to_string()).unwrap();

        // this also knows to estimate the `max_priority_fee_per_gas` but added it manually too
        let tx = TransactionRequest::new()
            .chain_id(BOTANIX_TESTNET.chain().id())
            .to(receiver_address)
            .value(amount)
            .gas_price(gas_price)
            .gas(U256::from(50_000));

        // send the tx with the initialized signer client
        let tx_receipt = self
            .client
            .send_transaction(tx, None)
            .await
            .map_err(Error::SignerMiddleware)?
            .await
            .map_err(Error::Provider)?;

        Ok(tx_receipt)
    }
}
