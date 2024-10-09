//! Defines structure for botanix RPC configurables and business logic

use std::{fmt, str::FromStr};

use bitcoincore_rpc::RpcApi;
use btcserverlib::extended_client::{GrpcClientError, GrpcClientFactory};
use reth_btc_wallet::bitcoind::{BitcoindClientFactory, BitcoindConfig, BitcoindFactory};
use reth_primitives::U256;
use revm_primitives::alloy_primitives::hex;
use tracing::error;
use url::Url;

/// Settings for the [BotanixConfig]
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BotanixConfig {
    /// Bitcoin network
    pub bitcoin_network: bitcoin::Network,

    /// The gRPC url for the bitcoin signer
    pub btc_server_factory: Option<GrpcClientFactory>,

    /// bitcoind configuration
    pub bitcoind_factory: BitcoindClientFactory,
}

impl Default for BotanixConfig {
    // Creates a mocked config. Do not use in production
    fn default() -> Self {
        BotanixConfig {
            bitcoin_network: bitcoin::Network::Regtest,
            btc_server_factory: None,
            // Use a public signet endpoint by default
            bitcoind_factory: BitcoindClientFactory::new(BitcoindConfig::new(
                "http://localhost:18443".parse::<Url>().expect("must be valid url address"),
                "foo".to_string(),
                "bar".to_string(),
            )),
        }
    }
}

impl BotanixConfig {
    /// Creates a new [BotanixConfig]
    pub fn new(
        bitcoin_network: bitcoin::Network,
        btc_server_factory: Option<GrpcClientFactory>,
        bitcoind_factory: BitcoindClientFactory,
    ) -> Self {
        BotanixConfig { bitcoin_network, btc_server_factory, bitcoind_factory }
    }
}

/// Errors from get gateway address RPC endpoint
#[derive(Debug)]
pub enum GatewayAddressRPCError {
    /// Failed to decode value received from `btc_server`
    FailedToDecodeAggregatePublicKey(hex::FromHexError),
    /// Invalid param received from client
    Client(GrpcClientError),
    /// Address generation failed
    FailedToGenerateGatewayAddress,
    /// Secp key conversion failed
    FailedToConvertPublicKey(secp256k1::Error),
    /// Address is generated for incorrect Network
    InvalidNetwork,
    /// Missing btc server connection information
    MissingBtcServerUrl,
    /// Rpc node does not have access to this resource
    ResourceAccess,
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
            GatewayAddressRPCError::ResourceAccess => {
                write!(f, "Resource access denied for this node")
            }
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
    FailedToGetEstimateSmartFee(bitcoincore_rpc::Error),
    /// Failed to initialize bitcoind client
    BitcoindClientInitialization,
    /// Failed to get estimate smart fee rate
    FailedToEstimateSmartFee(String),
}

impl fmt::Display for BtcFeeRateRPCError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BtcFeeRateRPCError::FailedToGetEstimateSmartFee(e) => {
                write!(f, "Failed to get estimate smart fee rate: {}", e)
            }
            BtcFeeRateRPCError::BitcoindClientInitialization => {
                write!(f, "Failed to initialize bitcoind client")
            }
            BtcFeeRateRPCError::FailedToEstimateSmartFee(e) => {
                write!(f, "Failed to estimate smart fee rate: {}", e)
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

impl Default for Botanix {
    fn default() -> Self {
        Self { botanix_rpc_config: Default::default() }
    }
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
        if self.botanix_rpc_config.btc_server_factory.is_none() {
            return Err(GatewayAddressRPCError::ResourceAccess);
        }

        let mut btc_server_client = self
            .botanix_rpc_config
            .btc_server_factory
            .clone()
            .expect("checked above")
            .build_and_connect()
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

        let bitcoind_client = self.botanix_rpc_config.bitcoind_factory.clone();
        let bitcoind_client = bitcoind_client
            .build_and_connect()
            .map_err(|_| MerkleProofRPCError::BitcoindClientInitialization)?;

        let block_hash = bitcoin::BlockHash::from_str(&block_hash)
            .map_err(|_e| MerkleProofRPCError::MalformedBlockHash)?;

        let txids = bitcoind_client
            .get_block_info(&block_hash)
            .map_err(|_e| MerkleProofRPCError::BitcoindClientInitialization)?
            .tx;

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
        let bitcoind_client = self.botanix_rpc_config.bitcoind_factory.clone();
        let bitcoind_client = bitcoind_client
            .build_and_connect()
            .map_err(|_| BtcFeeRateRPCError::BitcoindClientInitialization)?;
        let fee_result = bitcoind_client
            .estimate_smart_fee(1, None)
            .map_err(|e| BtcFeeRateRPCError::FailedToGetEstimateSmartFee(e))?;

        if let Some(fee) = fee_result.fee_rate {
            let sats_kb = bitcoin::FeeRate::from_sat_per_kwu(fee.to_sat() / 4);
            // this really doesn't need to be a U256 can be U64
            Ok(U256::from(sats_kb.to_sat_per_vb_ceil()))
        } else {
            // Use errors if available
            if let Some(errors) = fee_result.errors {
                let concatenated_errors = errors.join(", ");
                error!("Failed to get estimate smart fee rate: {}", concatenated_errors);
                Err(BtcFeeRateRPCError::FailedToEstimateSmartFee(concatenated_errors))
            } else {
                // else use default generic error
                Err(BtcFeeRateRPCError::FailedToEstimateSmartFee(
                    "Failed to get estimate smart fee rate".to_string(),
                ))
            }
        }
    }
}
