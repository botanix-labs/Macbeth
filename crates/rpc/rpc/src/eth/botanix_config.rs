//! Defines structure for botanix RPC configurables and business logic

use btc_wallet::address::gateway_address;
use secp256k1::PublicKey;
use serde::{Deserialize, Serialize};

lazy_static::lazy_static! {
    static ref SECP: secp256k1::Secp256k1<secp256k1::All> = secp256k1::Secp256k1::new();
}

/// Settings for the [BotanixConfig]
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct BotanixConfig {
    /// Bitcoin network
    pub bitcoin_network: bitcoin::Network,

    /// The gRPC url for the bitcoin signer
    pub btc_server_url: String,
}

impl Default for BotanixConfig {
    fn default() -> Self {
        BotanixConfig {
            bitcoin_network: bitcoin::Network::Signet,
            btc_server_url: "http://localhost:8080".to_string(),
        }
    }
}

impl BotanixConfig {
    fn new(bitcoin_network: bitcoin::Network, btc_server_url: String) -> Self {
        BotanixConfig { bitcoin_network, btc_server_url }
    }
}

#[derive(Debug)]
pub enum GatewayAddressRPCError {
    FailedToDecodeAggregatePublicKey(hex::FromHexError),
    InvalidParam(&'static str),
    FailedToGenerateGatewayAddress,
}

pub struct Botanix {
    botanix_rpc_config: BotanixConfig,
}

impl Botanix {
    /// Creates and returns instance of [Botanix]
    pub fn new(config: BotanixConfig) -> Self {
        Self { botanix_rpc_config: config }
    }

    /// Returns the configuration of botanix provider
    pub fn config(&self) -> &BotanixConfig {
        &self.botanix_rpc_config
    }

    pub async fn get_gateway_address(
        &self,
        eth_address: reth_primitives::Address,
        nonce: u64,
    ) -> std::result::Result<bitcoin::Address, GatewayAddressRPCError> {
        let mut client =
            client::BtcServerClient::connect(self.botanix_rpc_config.btc_server_url.clone())
                .await
                .unwrap();
        let request = tonic::Request::new(client::Empty {});

        let response = client.get_public_key(request).await.unwrap();
        let pk_hex = response.into_inner().publickey;

        let pk_vec = hex::decode(pk_hex)
            .map_err(|e| GatewayAddressRPCError::FailedToDecodeAggregatePublicKey(e))?;

        let pk = PublicKey::from_slice(pk_vec.as_slice()).map_err(|_e| {
            GatewayAddressRPCError::InvalidParam("Failed to derive aggregate public key from input")
        })?;
        let network = self.botanix_rpc_config.bitcoin_network;
        let address = gateway_address(&SECP, &pk, &eth_address, network, nonce)
            .map_err(|_e| GatewayAddressRPCError::FailedToGenerateGatewayAddress)?;

        Ok(address)
    }
}
