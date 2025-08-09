use crate::{create_psbt, encoding::PARSER};
use bitcoin::{
    address::{NetworkChecked, NetworkUnchecked},
    Address, Network, OutPoint,
};
use bitcoincore_rpc::jsonrpc::serde_json;
use botanix_storage::models::WalletSweepSession;
use btc_server_client::{
    AcceptWalletSweepSessionRequest, BtcServerExtendedApi, BtcServerExtendedClient,
};
use eyre::WrapErr;
use reth_primitives::Bytes;
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, fs, path::Path, str::FromStr, time::SystemTime};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletSweepRequest {
    // coordinator_id: u16,
    // coordinator_signature: Vec<u8>,
    pub destination_network: String,
    pub destination_address: Address<NetworkUnchecked>,
    // consensus_params: ConsensusParameters,
    // member_reports: Vec<MemberUtxoReport>,
    // consensus_utxos: Vec<ConsensusUtxoInfo>,
    // excluded_utxos: Vec<ExcludedUtxoInfo>,
    // consensus_stats: ConsensusStatistics,
    pub created_at: u64, // Unix timestamp in seconds since epoch
}

pub trait DestinationConfig: Debug {
    fn network(&self) -> eyre::Result<bitcoin::Network>;
    fn address(&self) -> eyre::Result<bitcoin::Address>;

    fn fee_rate(&self) -> eyre::Result<bitcoin::FeeRate>;
}

pub trait UtxoConfig: Debug {}

impl WalletSweepRequest {
    pub fn build() -> eyre::Result<Self> {
        // TODO: Accept all needed params and create request

        let created_at = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

        Ok(Self {
            destination_network: Network::Bitcoin.to_string(),
            destination_address: Address::from_str("bc1qexampleaddress1234567890abcdefg")?,
            created_at,
        })
    }

    pub async fn accept(
        &self,
        btc_server_client: &mut BtcServerExtendedClient,
    ) -> eyre::Result<()> {
        let rpc_request = AcceptWalletSweepSessionRequest { request: self.to_bytes().await? };

        btc_server_client.accept_wallet_sweep_session(rpc_request).await?;

        Ok(())
    }

    pub async fn to_bytes(&self) -> Result<Vec<u8>, eyre::Error> {
        PARSER.encode(self).await.wrap_err("Failed to encode WalletSweepRequest")
    }

    pub async fn from_bytes(bytes: &[u8]) -> Result<Self, eyre::Error> {
        PARSER.decode(bytes).await.wrap_err("Failed to decode WalletSweepRequest")
    }

    pub async fn from_json_file(path: &Path) -> Result<Self, eyre::Error> {
        let request_string = fs::read_to_string(path)
            .wrap_err_with(|| format!("Failed to read request file: {:?}", path))?;

        serde_json::from_str(&request_string)
            .wrap_err_with(|| format!("Failed to parse wallet sweep request from file: {:?}", path))
    }
}

#[derive(Serialize, Deserialize)]
pub struct ConsensusParameters {
    fee_rate_sat_vb: u64,
    utxo_ordering: String, // "lexicographic"
    threshold_percent: u8,
    reachable_members: Vec<u16>,
}

#[derive(Serialize, Deserialize)]
pub struct MemberUtxoReport {
    member_id: u16,
    utxos: Vec<Utxo>,
    timestamp: u64, // Unix timestamp in seconds since epoch
    member_signature: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
pub struct ConsensusUtxoInfo {
    outpoint: OutPoint,
    value_sat: u64,
    eth_address: Option<String>,
    version: u32,
    reported_by: Vec<u16>,
    consensus_percentage: u8,
}

#[derive(Serialize, Deserialize)]
pub struct ExcludedUtxoInfo {
    outpoint: OutPoint,
    value_sat: u64,
    eth_address: Option<String>,
    version: u32,
    reported_by: Vec<u16>,
    consensus_percentage: u8,
    exclusion_reason: String,
}

#[derive(Serialize, Deserialize)]
pub struct ConsensusStatistics {
    total_members: u8,
    reachable_members: u8,
    offline_members: Vec<u16>,
    consensus_utxos_count: u32,
    excluded_utxos_count: u32,
    total_value_sat: u64,
    excluded_value_sat: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Utxo {}

impl TryFrom<btc_server_client::Utxo> for Utxo {
    type Error = eyre::Error;

    fn try_from(_value: btc_server_client::Utxo) -> Result<Self, Self::Error> {
        // TODO: Implement conversion logic here
        Ok(Utxo {})
    }
}

impl TryInto<WalletSweepSession> for WalletSweepRequest {
    type Error = eyre::Error;

    fn try_into(self) -> Result<WalletSweepSession, Self::Error> {
        let psbt = create_psbt(self.clone())?;

        // TODO: Implement conversion logic here
        let session = WalletSweepSession {
            psbt_bytes: psbt.serialize().into(), // Construct PSBT here?
            bitcoin_network: self.destination_network.parse()?,
            bitcoin_destination_address: self.destination_address,
            created_at: self.created_at,
        };

        Ok(session)
    }
}
