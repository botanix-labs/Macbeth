//! Defines structure for botanix RPC configurables and business logic

use alloy_primitives::hex;
use client::GetPublicKeyRequest;
use reth_btc_wallet::bitcoind::{BitcoindClient, BitcoindConfig};
use reth_primitives::U256;
use secp256k1::PublicKey;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use url::Url;

/// Settings for the [BotanixConfig]
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct BotanixConfig {
    /// Bitcoin network
    pub bitcoin_network: bitcoin::Network,

    // TODO set up stronger url types
    /// The gRPC url for the bitcoin signer
    pub btc_server: String,

    /// bitcoind configuration
    pub bitcoind_config: BitcoindConfig,
}

impl Default for BotanixConfig {
    fn default() -> Self {
        BotanixConfig {
            bitcoin_network: bitcoin::Network::Signet,
            btc_server: "http://localhost:8080".to_string(),
            // Use a public signet endpoint by default
            bitcoind_config: BitcoindConfig::new(
                "https://bitcoind.botanixlabs.dev"
                    .parse::<Url>()
                    .expect("must be valid url address"),
                "usr".to_string(),
                "pwd".to_string(),
            ),
        }
    }
}

impl BotanixConfig {
    //  TODO (armins) bitcoin network should be a Arc<dyn BlockSource>
    #[allow(dead_code)]
    fn new(
        bitcoin_network: bitcoin::Network,
        btc_server: String,
        bitcoind_username: String,
        bitcoind_password: String,
    ) -> Self {
        // TODO(armins) Update these to point to botanix mempool instances
        let bitcoind_url = match bitcoin_network {
            bitcoin::Network::Bitcoin => "https://bitcoind.botanixlabs.dev", // TODO: update this
            bitcoin::Network::Testnet => "https://bitcoind.botanixlabs.dev", // TODO: update this
            bitcoin::Network::Signet => "https://bitcoind.botanixlabs.dev/", // TODO: update this
            _ => panic!("Unsupported network"),
        };

        BotanixConfig {
            bitcoin_network,
            btc_server,
            bitcoind_config: BitcoindConfig::new(
                bitcoind_url.parse::<Url>().expect("must be valid ip address"),
                bitcoind_username,
                bitcoind_password,
            ),
        }
    }

    /// Set btc server Grpc Url
    pub fn btc_server(mut self, btc_server: String) -> Self {
        self.btc_server = btc_server;
        self
    }

    /// Set mempool space block source url
    pub fn bitcoind(mut self, url: Url, username: String, password: String) -> Self {
        self.bitcoind_config = BitcoindConfig::new(url, username, password);
        self
    }
}

/// Errors from get gateway address RPC endpoint
#[derive(Debug)]
pub enum GatewayAddressRPCError {
    /// Failed to decode value recieved from `btc_server`
    FailedToDecodeAggregatePublicKey(hex::FromHexError),
    /// Invalid param recieved from client
    InvalidParam(tonic::Status),
    /// Address generation failed
    FailedToGenerateGatewayAddress,
    /// Secp key conversion failed
    FailedToConvertPublicKey(secp256k1::Error),
    /// Address is generated for incorrect Network
    InvalidNetwork,
}

/// Errors from get merkle proof RPC endpoint
#[derive(Debug)]
pub enum MerkleProofRPCError {
    /// Incorrect txid format
    InvalidTxId,
    /// Failed to get txids from blockhash
    FailedToGetTxIds,
    /// txid not in block
    TxIdNotInBlock,
    /// Failed to encode Partial Merkle Tree
    FailedToEncodePartialMerkleTree(bitcoin::consensus::encode::Error),
    /// Malformed block hash
    MalformedBlockHash,
    /// Bitcoin client initialization
    BitcoindClientInitialization,
}

/// Errors from get btc fees RPC endpoint
#[derive(Debug)]
pub enum BtcFeesRPCError {}

impl fmt::Display for MerkleProofRPCError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MerkleProofRPCError::InvalidTxId => write!(f, "Invalid txid format"),
            MerkleProofRPCError::FailedToGetTxIds => {
                write!(f, "Failed to get txids from blockhash")
            }
            MerkleProofRPCError::TxIdNotInBlock => write!(f, "Txid not in block"),
            MerkleProofRPCError::FailedToEncodePartialMerkleTree(e) => {
                write!(f, "Failed to encode Partial Merkle Tree: {}", e)
            }
            MerkleProofRPCError::MalformedBlockHash => write!(f, "Malformed block hash"),
            MerkleProofRPCError::BitcoindClientInitialization => {
                write!(f, "Bad bitcoind client initialization")
            }
        }
    }
}

impl std::error::Error for MerkleProofRPCError {}

impl From<MerkleProofRPCError> for String {
    fn from(error: MerkleProofRPCError) -> Self {
        error.to_string()
    }
}

/// Botanix config
#[derive(Debug)]
pub struct Botanix {
    /// Botanix config
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

    /// Function calls btc_server to get "aggregated public key" and generated taproot gateway
    /// address
    pub async fn get_gateway_address(
        &self,
        eth_address: reth_primitives::Address,
    ) -> std::result::Result<(bitcoin::Address, secp256k1::PublicKey), GatewayAddressRPCError> {
        let mut client =
            client::BtcServerClient::connect(self.botanix_rpc_config.btc_server.clone())
                .await
                .unwrap();
        let request = tonic::Request::new(client::GetGatewayAddressRequest {
            eth_address: eth_address.to_string(),
        });

        let response = client
            .get_gateway_address(request)
            .await
            .map_err(|e| GatewayAddressRPCError::InvalidParam(e))?
            .into_inner();

        let address = bitcoin::Address::from_str(response.gateway_address.as_str())
            .map_err(|_e| GatewayAddressRPCError::FailedToGenerateGatewayAddress)?
            .assume_checked();

        if address.network != self.botanix_rpc_config.bitcoin_network {
            return Err(GatewayAddressRPCError::InvalidNetwork);
        }

        let pk = secp256k1::PublicKey::from_slice(
            &hex::decode(response.publickey.as_str())
                .map_err(GatewayAddressRPCError::FailedToDecodeAggregatePublicKey)?,
        )
        .map_err(|e| GatewayAddressRPCError::FailedToConvertPublicKey(e))?;

        Ok((address, pk))
    }

    /// Function generates merkle proof for txid in a given block
    pub async fn get_merkle_proof(
        &self,
        txid: String,
        block_hash: String,
    ) -> std::result::Result<Vec<u8>, MerkleProofRPCError> {
        let tx_id: bitcoin::Txid = bitcoin::Txid::from_str(txid.as_str())
            .map_err(|_e| MerkleProofRPCError::InvalidTxId)?;
        let bitcoind_client = BitcoindClient::new(self.config().bitcoind_config.clone())
            .map_err(|_| MerkleProofRPCError::BitcoindClientInitialization)?;

        let txids = bitcoind_client
            .get_txids(
                bitcoin::BlockHash::from_str(&block_hash)
                    .map_err(|_e| MerkleProofRPCError::MalformedBlockHash)?,
            )
            .await
            .map_err(|_e| MerkleProofRPCError::FailedToGetTxIds)?;
        if !txids.contains(&tx_id) {
            return Err(MerkleProofRPCError::TxIdNotInBlock);
        }

        let matches = txids.iter().map(|txid| txid == &tx_id).collect::<Vec<_>>();

        let pmt = bitcoin::merkle_tree::PartialMerkleTree::from_txids(&txids, &matches);
        Ok(bitcoin::consensus::serialize(&pmt))
    }

    /// Function calls btc_server to get btc fee rate in sat/vb for a pegout transaction.
    ///
    /// Currently returns a static fee rate without calling btc_server.
    pub async fn get_btc_fee_rate(&self) -> std::result::Result<U256, BtcFeesRPCError> {
        Ok(U256::from(30u32))
    }
}
