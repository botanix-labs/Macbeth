use tracing::info;

use ethers::{
    core::k256::ecdsa::SigningKey,
    prelude::*,
    providers::{Http, Provider},
    signers::LocalWallet,
    utils::{self},
};
use reth_primitives::{public_key_to_address, Address, BOTANIX_TESTNET};
use secp256k1::SecretKey;
use std::str::FromStr;

#[derive(Debug)]
pub struct TestPayloadSender {
    pub client: SignerMiddleware<Provider<Http>, Wallet<SigningKey>>,
    pub sender_address: Address,
}

impl TestPayloadSender {
    pub async fn new(rpc_port: u16, sender_secret_key: &str) -> Self {
        // Connect to the network
        let provider =
            Provider::<Http>::try_from(&format!("http://127.0.0.1:{}", rpc_port)).unwrap();
        info!("Node URL: {}", &format!("http://127.0.0.1:{}", rpc_port));

        // get chain id
        let chain_id = provider.get_chainid().await.unwrap();
        assert!(U256::from(BOTANIX_TESTNET.chain().id()) == chain_id, "expected same chain id");

        // get the sender address
        let secp_sender_secret_key = SecretKey::from_str(sender_secret_key).unwrap();
        let secp_sender_pub_key = secp256k1::PublicKey::from_secret_key(
            &secp256k1::Secp256k1::new(),
            &secp_sender_secret_key,
        );
        let sender_address = public_key_to_address(secp_sender_pub_key);

        // create a local wallet
        let wallet: LocalWallet =
            sender_secret_key.parse::<LocalWallet>().unwrap().with_chain_id(chain_id.as_u64());

        // connect the wallet to the provider
        let client = SignerMiddleware::new(provider, wallet);

        Self { client, sender_address }
    }

    pub async fn send(
        &self,
        receiver_address: &str,
        amount_botanix: u64,
    ) -> Result<TxHash, &'static str> {
        // get current receiver balance
        let receiver_account = NameOrAddress::from_str(receiver_address).unwrap();
        let receiver_cur_balance = self.client.get_balance(receiver_account, None).await.unwrap();
        println!("Receiver current balance: {:?}", receiver_cur_balance.to_string());

        // get current sender balance
        let sender_account = NameOrAddress::from_str(&self.sender_address.to_string()).unwrap();
        let sender_cur_balance = self.client.get_balance(sender_account, None).await.unwrap();
        println!("Sender current balance: {:?}", sender_cur_balance.to_string());

        // this also knows to estimate the `max_priority_fee_per_gas` but added it manually too
        let tx = Eip1559TransactionRequest::new()
            .to(receiver_address)
            .value(U256::from(utils::parse_ether(amount_botanix).unwrap()))
            .max_priority_fee_per_gas(U256::from(2000000000_u128)); // 2 Gwei

        // send the tx with the initialized signer client
        let pending_tx = self.client.send_transaction(tx, None).await.unwrap();
        Ok(pending_tx.tx_hash())
    }

    pub async fn send_invalid(&self, receiver_address: &str) {
        let tx = Eip1559TransactionRequest::new()
            .to(receiver_address)
            .value(utils::parse_ether(0).unwrap())
            .max_priority_fee_per_gas(U256::from(2000000000_u128)) // 2 GWEI
            .nonce(0_u64); // nonce should be 1 when invoked

        // send the tx with the initialized signer client
        let err = self
            .client
            .send_transaction(tx, None)
            .await
            .expect_err("should fail with nonce too low");
        println!("Error: {:?}", err);
    }
}
