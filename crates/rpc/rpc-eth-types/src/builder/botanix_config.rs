//! Defines structure for botanix RPC configurables and business logic

use std::{fmt, str::FromStr};

use revm_primitives::alloy_primitives::hex;
use btcserverlib::extended_client::{BtcServerExtendedClient, GrpcClientError};
use client::jwt::JwtSecret;
use reth_btc_wallet::bitcoind::{BitcoindClient, BitcoindConfig, BitcoindError};
use reth_primitives::U256;
use serde::{Deserialize, Serialize};
use tracing::error;
use url::Url;

/// Settings for the [BotanixConfig]
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct BotanixConfig {
    /// Bitcoin network
    pub bitcoin_network: bitcoin::Network,

    // TODO set up stronger url types
    /// The gRPC url for the bitcoin signer
    pub btc_server: Option<String>,

    /// bitcoind configuration
    pub bitcoind_config: BitcoindConfig,

    /// Jwt btc-server authentication secret
    pub btc_server_jwt_secret: Option<JwtSecret>,
}

impl Default for BotanixConfig {
    fn default() -> Self {
        BotanixConfig {
            bitcoin_network: bitcoin::Network::Regtest,
            btc_server: Some("http://localhost:8080".to_string()),
            // Use a public signet endpoint by default
            bitcoind_config: BitcoindConfig::new(
                "http://localhost:18443".parse::<Url>().expect("must be valid url address"),
                "foo".to_string(),
                "bar".to_string(),
            ),
            btc_server_jwt_secret: None,
        }
    }
}

impl BotanixConfig {
    //  TODO (armins) bitcoin network should be a Arc<dyn BlockSource>
    #[allow(dead_code)]
    fn new(
        bitcoin_network: bitcoin::Network,
        btc_server: Option<String>,
        bitcoind_username: String,
        bitcoind_password: String,
        btc_server_jwt_secret: Option<JwtSecret>,
    ) -> Self {
        // TODO(armins) Update these to point to botanix mempool instances
        let bitcoind_url = match bitcoin_network {
            bitcoin::Network::Bitcoin => "https://bitcoind.botanixlabs.dev", // TODO: update this
            bitcoin::Network::Testnet => "https://bitcoind.botanixlabs.dev", // TODO: update this
            bitcoin::Network::Signet => "https://bitcoind.botanixlabs.dev/", // TODO: update this
            bitcoin::Network::Regtest => "http://localhost:18443",           /* local regetest */
            // network
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
            btc_server_jwt_secret,
        }
    }

    /// Set btc server Grpc Url
    pub fn btc_server(mut self, btc_server: Option<String>) -> Self {
        self.btc_server = btc_server;
        self
    }

    /// Set btc server jwt secret
    pub fn btc_server_jwt_secret(mut self, btc_server_jwt_secret: Option<JwtSecret>) -> Self {
        self.btc_server_jwt_secret = btc_server_jwt_secret;
        self
    }

    /// Set bitcoin network
    pub fn bitcoin_network(mut self, bitcoin_network: bitcoin::Network) -> Self {
        self.bitcoin_network = bitcoin_network;
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
    Client(GrpcClientError),
    /// Address generation failed
    FailedToGenerateGatewayAddress,
    /// Secp key conversion failed
    FailedToConvertPublicKey(secp256k1::Error),
    /// Address is generated for incorrect Network
    InvalidNetwork,
    /// Missing btc server connection information
    MissingBtcServerUrl,
}

impl fmt::Display for GatewayAddressRPCError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GatewayAddressRPCError::FailedToDecodeAggregatePublicKey(e) => {
                write!(f, "Failed to decode aggregate public key: {}", e)
            }
            GatewayAddressRPCError::MissingBtcServerUrl => {
                write!(f, "Missing Btc Server Connection Url")
            }
            GatewayAddressRPCError::FailedToGenerateGatewayAddress => {
                write!(f, "Failed to generate gateway address")
            }
            GatewayAddressRPCError::FailedToConvertPublicKey(e) => {
                write!(f, "Failed to convert public key: {}", e)
            }
            GatewayAddressRPCError::InvalidNetwork => write!(f, "Invalid network"),
            GatewayAddressRPCError::Client(e) => write!(f, "Grpc client error: {}", e),
        }
    }
}

impl From<GatewayAddressRPCError> for String {
    fn from(error: GatewayAddressRPCError) -> Self {
        error.to_string()
    }
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

/// Error from get btc fee rate RPC endpoint
#[derive(Debug)]
pub enum BtcFeeRateRPCError {
    /// Failed to get estimate smart fee rate
    FailedToGetEstimateSmartFee(BitcoindError),
}

impl fmt::Display for BtcFeeRateRPCError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BtcFeeRateRPCError::FailedToGetEstimateSmartFee(e) => {
                write!(f, "Failed to get estimate smart fee rate: {}", e)
            }
        }
    }
}

/// Botanix config
#[derive(Clone, Debug)]
pub struct Botanix {
    /// Botanix config
    pub botanix_rpc_config: BotanixConfig,
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
        // Non-federation nodes will not have btc_server set
        let btc_server_address = self
            .botanix_rpc_config
            .btc_server
            .clone()
            .ok_or_else(|| GatewayAddressRPCError::MissingBtcServerUrl)?;

        let mut btc_server_client = BtcServerExtendedClient::new(
            btc_server_address,
            self.botanix_rpc_config.btc_server_jwt_secret.clone(),
        )
        .await
        .map_err(GatewayAddressRPCError::Client)?;

        let request = client::GetGatewayAddressRequest { eth_address: eth_address.to_string() };

        let response = btc_server_client
            .get_gateway_address(request)
            .await
            .map_err(GatewayAddressRPCError::Client)?;

        let address = bitcoin::Address::from_str(response.gateway_address.as_str())
            .map_err(|_e| GatewayAddressRPCError::FailedToGenerateGatewayAddress)?
            .require_network(self.botanix_rpc_config.bitcoin_network)
            .map_err(|_e| GatewayAddressRPCError::InvalidNetwork)?;

        let pk = secp256k1::PublicKey::from_slice(
            &hex::decode(response.publickey.as_str())
                .map_err(GatewayAddressRPCError::FailedToDecodeAggregatePublicKey)?,
        )
        .map_err(GatewayAddressRPCError::FailedToConvertPublicKey)?;

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
        // TODO replace this with the jsonrpc client for bitcoind
        let bitcoind_client = BitcoindClient::new(self.config().bitcoind_config.clone())
            .map_err(|_| MerkleProofRPCError::BitcoindClientInitialization)?;

        let txids = bitcoind_client
            .get_txids(
                bitcoin::BlockHash::from_str(&block_hash)
                    .map_err(|_e| MerkleProofRPCError::MalformedBlockHash)?,
            )
            .map_err(|_e| MerkleProofRPCError::FailedToGetTxIds)?;
        if !txids.contains(&tx_id) {
            return Err(MerkleProofRPCError::TxIdNotInBlock);
        }

        let matches = txids.iter().map(|txid| txid == &tx_id).collect::<Vec<_>>();

        let pmt = bitcoin::merkle_tree::PartialMerkleTree::from_txids(&txids, &matches);
        Ok(bitcoin::consensus::serialize(&pmt))
    }

    /// Function calls btc_server to get btc fee rate in BTC/kB for a pegout transaction.
    ///
    /// Converts fee rate to sat/vB and returns it.
    pub async fn get_btc_fee_rate(&self) -> std::result::Result<U256, BtcFeeRateRPCError> {
        let bitcoind_client = BitcoindClient::new(self.config().bitcoind_config.clone())
            .map_err(BtcFeeRateRPCError::FailedToGetEstimateSmartFee)?;
        let fee_result = bitcoind_client
            .get_estimate_smart_fee()
            .map_err(BtcFeeRateRPCError::FailedToGetEstimateSmartFee)?;

        if let Some(fee) = fee_result.fee_rate {
            let sats_kb = bitcoin::FeeRate::from_sat_per_kwu(fee.to_sat() / 4);
            // this really doesnt need to be a U256 can be U64
            Ok(U256::from(sats_kb.to_sat_per_vb_ceil()))
        } else {
            // Use errors if available
            if let Some(errors) = fee_result.errors {
                let concatenated_errors = errors.join(", ");
                error!("Failed to get estimate smart fee rate: {}", concatenated_errors);
                Err(BtcFeeRateRPCError::FailedToGetEstimateSmartFee(
                    BitcoindError::EstimateSmartFeeFailed(bitcoincore_rpc::Error::ReturnedError(
                        concatenated_errors,
                    )),
                ))
            } else {
                // else use default generic error
                Err(BtcFeeRateRPCError::FailedToGetEstimateSmartFee(
                    BitcoindError::EstimateSmartFeeFailed(bitcoincore_rpc::Error::ReturnedError(
                        "Failed to get estimate smart fee rate".to_string(),
                    )),
                ))
            }
        }
    }
}
