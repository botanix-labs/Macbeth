use crate::errors::WalletError;
use async_trait::async_trait;
use ethers::{prelude::*, types::transaction::eip2718::TypedTransaction};
use std::{str::FromStr, sync::Arc};
/// wallet trait
#[async_trait]
pub trait Wallet {
    /// getbalnce  for given address
    async fn get_balance(&self, sceret_key: String) -> Result<U256, WalletError>;
    /// transfer balance  for given account
    async fn sweep_balance(&self, sceret_key: String, to: Address) -> Result<TxHash, WalletError>;
    /// get transaction details  for given tx_hash
    async fn get_transaction_info(&self, tx_hash: TxHash) -> Result<Transaction, WalletError>;
}

/// rpc client struct
#[derive(Debug)]
pub struct Client {
    /// Shared HTTP provider client
    pub client: Arc<Provider<Http>>,
    /// Chain ID for the blockchain network
    pub chain_id: u64,
    /// URL of the blockchain provider
    pub provider_url: String,
}
impl Client {
    /// Asynchronously creates a new `Client` instance with the given `url` and `chain_id`,
    /// returning a `Result<Client, WalletError>`.
    pub async fn new(url: &str, chain_id: u64) -> Result<Self, WalletError> {
        let provider = Provider::<Http>::try_from(url)
            .map_err(|e| WalletError::IoError(format!("{:?}", e)))?;

        let client = Self { client: Arc::new(provider), chain_id, provider_url: url.to_string() };

        Ok(client)
    }
}
#[async_trait]
impl Wallet for Client {
    /// get balance for given address
    async fn get_balance(&self, secret_key: String) -> Result<U256, WalletError> {
        let wallet: LocalWallet = secret_key.parse().map_err(|e| {
            WalletError::CustomError(format!("Failed to parse secret key: {:?}", e))
        })?;

        let from_address = wallet.address();
        self.client
            .get_balance(from_address, None)
            .await
            .map_err(|e| WalletError::RpcError(e.to_string()))
    }

    /// sweep balance from one account to another account
    async fn sweep_balance(&self, secret_key: String, to: Address) -> Result<TxHash, WalletError> {
        let provider = Arc::clone(&self.client);

        let wallet: LocalWallet = secret_key.parse().map_err(|e| {
            WalletError::CustomError(format!("Failed to parse secret key: {:?}", e))
        })?;

        let from_address = wallet.address();
        let balance = self.get_balance(secret_key).await?;

        if balance.is_zero() {
            return Err(WalletError::RpcError("Insufficient funds.".to_string()));
        }

        let tx: TypedTransaction = TransactionRequest::new().to(to).from(from_address).into();

        let estimated_gas = provider
            .estimate_gas(&tx, None)
            .await
            .map_err(|e| WalletError::GasError(format!("Gas estimation failed: {}", e)))?;

        let gas_price = provider
            .get_gas_price()
            .await
            .map_err(|e| WalletError::GasError(format!("Failed to get gas price: {}", e)))?;

        let chain_id: u64 = if self.chain_id == 0 {
            provider
                .get_chainid()
                .await
                .map_err(|e| WalletError::GasError(format!("Failed to get chain Id: {}", e)))?
                .as_u64()
        } else {
            self.chain_id
        };

        let total_gas_fee = estimated_gas
            .checked_mul(gas_price)
            .ok_or_else(|| WalletError::GasError("Overflow in gas fee calculation".to_string()))?;

        if balance <= total_gas_fee {
            return Err(WalletError::CustomError(
                "Insufficient balance to cover gas fees".to_string(),
            ));
        }

        let nonce = provider
            .get_transaction_count(wallet.address(), None)
            .await
            .map_err(|e| WalletError::RpcError(format!("Failed to get nonce: {}", e)))?;

        let send_amount = balance.checked_sub(total_gas_fee).ok_or_else(|| {
            WalletError::CustomError("Insufficient balance to cover gas fees".to_string())
        })?;

        let mut tx: TypedTransaction = TransactionRequest::new()
            .to(to)
            .value(send_amount)
            .from(from_address)
            .nonce(nonce)
            .into();
        tx.set_chain_id(chain_id);
        tx.set_gas(estimated_gas);
        tx.set_gas_price(gas_price);

        let signature = wallet
            .sign_transaction(&tx)
            .await
            .map_err(|e| WalletError::CustomError(e.to_string()))?;

        let signed_tx_bytes: Bytes = tx.rlp_signed(&signature);

        let pending_tx = provider
            .send_raw_transaction(signed_tx_bytes)
            .await
            .map_err(|e| WalletError::CustomError(e.to_string()))?;

        Ok(pending_tx.tx_hash())
    }

    //get transactions details
    async fn get_transaction_info(&self, tx_hash: TxHash) -> Result<Transaction, WalletError> {
        match self.client.get_transaction(tx_hash).await {
            Ok(Some(transaction)) => Ok(transaction),
            Ok(None) => Err(WalletError::TransactionNotFound("Transaction not found".to_string())),
            Err(e) => Err(WalletError::RpcError(e.to_string())),
        }
    }
}
