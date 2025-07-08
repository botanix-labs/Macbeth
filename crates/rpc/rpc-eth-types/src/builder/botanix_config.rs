//! Defines structure for botanix RPC configurables and business logic

use std::{fmt, str::FromStr};

use bitcoincore_rpc::RpcApi;
use botanix_btc_wallet::bitcoind::{BitcoindClientFactory, BitcoindConfig, BitcoindFactory};
use frost_secp256k1_tr::{self as frost};

use btcserverlib::wallet::address::{generate_taproot_address, generate_tweaked_public_key};

use reth_primitives::{header_ext::HeaderExt, U256};
use reth_storage_api::BlockReaderIdExt;
use thiserror::Error;
use tracing::error;
use url::Url;

/// Settings for the [`BotanixConfig`]
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BotanixConfig {
    /// Bitcoin network
    pub bitcoin_network: bitcoin::Network,

    /// bitcoind configuration
    pub bitcoind_factory: BitcoindClientFactory,
}

impl Default for BotanixConfig {
    // Creates a mocked config. Do not use in production
    fn default() -> Self {
        Self {
            bitcoin_network: bitcoin::Network::Regtest,
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
    /// Creates a new [`BotanixConfig`]
    pub const fn new(
        bitcoin_network: bitcoin::Network,
        bitcoind_factory: BitcoindClientFactory,
    ) -> Self {
        Self { bitcoin_network, bitcoind_factory }
    }
}

/// Errors from get gateway address RPC endpoint
#[derive(Debug, Error)]
pub enum GatewayAddressRPCError {
    /// Failed to get latest header
    #[error("Failed to get latest header")]
    FailedToGetLatestHeader,
    /// Cannot calculate gateway address for genesis block
    #[error("Cannot calculate gateway address for genesis block")]
    GenesisBlock,
    /// Failed to deserialize aggregated public key
    #[error("Frost deserialization failed {0}")]
    FailedToDeserializeAggregatedPublicKey(#[from] frost::Error),
    /// Failed to tweak the public key
    #[error("Failed to tweak the public key")]
    FailedToTweakPublicKey,
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
            Self::InvalidTxId => write!(f, "Invalid txid format"),
            Self::FailedToGetTxIds => {
                write!(f, "Failed to get txids from blockhash")
            }
            Self::TxIdNotInBlock => write!(f, "Txid not in block"),
            Self::FailedToEncodePartialMerkleTree(e) => {
                write!(f, "Failed to encode Partial Merkle Tree: {}", e)
            }
            Self::MalformedBlockHash => write!(f, "Malformed block hash"),
            Self::BitcoindClientInitialization => {
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
            Self::FailedToGetEstimateSmartFee(e) => {
                write!(f, "Failed to get estimate smart fee rate: {}", e)
            }
            Self::BitcoindClientInitialization => {
                write!(f, "Failed to initialize bitcoind client")
            }
            Self::FailedToEstimateSmartFee(e) => {
                write!(f, "Failed to estimate smart fee rate: {}", e)
            }
        }
    }
}

/// Botanix config
#[derive(Clone, Debug, Default)]
pub struct Botanix {
    /// Botanix config
    pub botanix_rpc_config: BotanixConfig,
}

impl Botanix {
    /// Creates and returns instance of [Botanix]
    pub const fn new(config: BotanixConfig) -> Self {
        Self { botanix_rpc_config: config }
    }

    /// Returns the configuration of botanix provider
    pub const fn config(&self) -> &BotanixConfig {
        &self.botanix_rpc_config
    }

    /// Function calls "`get_aggregate_public_key`"
    pub async fn get_aggregate_public_key(
        &self,
        provider: &impl BlockReaderIdExt,
    ) -> std::result::Result<secp256k1::PublicKey, GatewayAddressRPCError> {
        let latest_header = provider
            .latest_header()
            .map_err(|_| GatewayAddressRPCError::FailedToGetLatestHeader)?
            .ok_or(GatewayAddressRPCError::FailedToGetLatestHeader)?;

        if latest_header.number == 0 {
            return Err(GatewayAddressRPCError::GenesisBlock);
        }

        let agg_pk = latest_header
            .deserialize_extra_data_header()
            .map_err(|_| GatewayAddressRPCError::FailedToGetLatestHeader)?
            .aggregated_public_key;

        Ok(agg_pk)
    }

    /// Function calls `btc_server` to get "aggregated public key" and generated taproot gateway
    /// address
    pub async fn get_gateway_address(
        &self,
        eth_address: reth_primitives::Address,
        provider: &impl BlockReaderIdExt,
    ) -> std::result::Result<(bitcoin::Address, secp256k1::PublicKey), GatewayAddressRPCError> {
        let eth_address_bytes = eth_address.0 .0;
        let latest_header = provider
            .latest_header()
            .map_err(|_| GatewayAddressRPCError::FailedToGetLatestHeader)?
            .ok_or(GatewayAddressRPCError::FailedToGetLatestHeader)?;

        if latest_header.number == 0 {
            return Err(GatewayAddressRPCError::GenesisBlock);
        }
        // We need to tweak the aggregated public key with the eth address
        let agg_pk = latest_header
            .deserialize_extra_data_header()
            .map_err(|_| GatewayAddressRPCError::FailedToGetLatestHeader)?
            .aggregated_public_key;

        let vpk = frost::VerifyingKey::deserialize(&agg_pk.serialize())?;
        let tweaked_pk = generate_tweaked_public_key(&vpk, &eth_address_bytes)
            .map_err(|_| GatewayAddressRPCError::FailedToTweakPublicKey)?;
        let address =
            generate_taproot_address(&tweaked_pk, self.botanix_rpc_config.bitcoin_network);

        Ok((address, agg_pk))
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

    /// Function calls `btc_server` to get btc fee rate in BTC/kB for a pegout transaction.
    ///
    /// Converts fee rate to sat/vB and returns it.
    pub async fn get_btc_fee_rate(&self) -> std::result::Result<U256, BtcFeeRateRPCError> {
        let bitcoind_client = self.botanix_rpc_config.bitcoind_factory.clone();
        let bitcoind_client = bitcoind_client
            .build_and_connect()
            .map_err(|_| BtcFeeRateRPCError::BitcoindClientInitialization)?;
        let fee_result = bitcoind_client
            .estimate_smart_fee(1, None)
            .map_err(BtcFeeRateRPCError::FailedToGetEstimateSmartFee)?;

        if let Some(fee) = fee_result.fee_rate {
            // Conversion formula
            let sat_per_vb = fee.to_float_in(bitcoin::Denomination::Bitcoin) * 100_000.0;
            // this really doesn't need to be a U256 can be U64
            Ok(U256::from(sat_per_vb.ceil() as u64))
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
