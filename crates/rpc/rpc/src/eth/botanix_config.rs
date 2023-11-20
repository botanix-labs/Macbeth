//! Defines structure for botanix RPC configurables and business logic

use std::{fmt, str::FromStr};

use btc_wallet::block_source::BlockSource;
use reth_primitives::U256;
use secp256k1::PublicKey;
use serde::{Deserialize, Serialize};

// TODO Secp should be getting pulled from provider
lazy_static::lazy_static! {
    static ref SECP: secp256k1::Secp256k1<secp256k1::All> = secp256k1::Secp256k1::new();
}

/// Settings for the [BotanixConfig]
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct BotanixConfig {
    /// Bitcoin network
    pub bitcoin_network: bitcoin::Network,

    // TODO set up stronger url types
    /// The gRPC url for the bitcoin signer
    pub btc_server: String,

    /// mempool space url
    pub mempool_space_url: String,
}

impl Default for BotanixConfig {
    fn default() -> Self {
        BotanixConfig {
            bitcoin_network: bitcoin::Network::Signet,
            btc_server: "http://localhost:8080".to_string(),
            // Use a public signet endpoint by default
            mempool_space_url: "https://mempool.space/signet/api".to_string(),
        }
    }
}

impl BotanixConfig {
    //  TODO (armins) bitcoin network should be a Arc<dyn BlockSource>
    fn new(bitcoin_network: bitcoin::Network, btc_server: String) -> Self {
        // TODO(armins) Update these to point to botanix mempool instances
        let mempool_space_api = match bitcoin_network {
            bitcoin::Network::Bitcoin => "https://mempool.space/api",
            bitcoin::Network::Testnet => "https://mempool.space/api/testnet",
            bitcoin::Network::Signet => "https://mempool.space/api/signet",
            _ => panic!("Unsupported network"),
        };

        BotanixConfig {
            bitcoin_network,
            btc_server,
            mempool_space_url: mempool_space_api.to_string(),
        }
    }

    /// Set btc server Grpc Url
    pub fn btc_server(mut self, btc_server: String) -> Self {
        self.btc_server = btc_server;
        self
    }

    /// Set mempool space block source url
    pub fn mempool_space_url(mut self, mempool_space_url: String) -> Self {
        self.mempool_space_url = mempool_space_url;
        self
    }
}

/// Errors from get gateway address RPC endpoint
#[derive(Debug)]
pub enum GatewayAddressRPCError {
    /// Failed to decode value recieved from `btc_server`
    FailedToDecodeAggregatePublicKey(hex::FromHexError),
    /// Invalid param recieved from client
    InvalidParam(&'static str),
    /// Address generation failed
    FailedToGenerateGatewayAddress,
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
        nonce: u64,
    ) -> std::result::Result<(bitcoin::Address, secp256k1::PublicKey), GatewayAddressRPCError> {
        let mut client =
            client::BtcServerClient::connect(self.botanix_rpc_config.btc_server.clone())
                .await
                .unwrap();
        let request = tonic::Request::new(client::Empty {});

        let response = client.get_public_key(request).await.unwrap();
        let pk_hex = response.into_inner().publickey;

        let pk = PublicKey::from_str(pk_hex.as_str()).map_err(|_e| {
            GatewayAddressRPCError::InvalidParam("Failed to derive aggregate public key from input")
        })?;
        let network = self.botanix_rpc_config.bitcoin_network;
        let address = btc_wallet::address::gateway_address(
            &SECP,
            &pk,
            &eth_address.as_slice().to_vec(),
            network,
            nonce,
        )
        .map_err(|_e| GatewayAddressRPCError::FailedToGenerateGatewayAddress)?;

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
        let mempool =
            btc_wallet::block_source::MempoolSpace::new(self.config().mempool_space_url.clone());

        let txids = mempool
            .get_txids(
                bitcoin::BlockHash::from_str(&block_hash)
                    .map_err(|_e| MerkleProofRPCError::MalformedBlockHash)?,
            )
            .await
            .map_err(|_e| MerkleProofRPCError::FailedToGetTxIds)?;
        if !txids.contains(&tx_id) {
            return Err(MerkleProofRPCError::TxIdNotInBlock)
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
