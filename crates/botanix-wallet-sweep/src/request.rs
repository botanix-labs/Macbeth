use bitcoin::OutPoint;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct WalletSweepRequest {
    session_id: [u8; 32],
    coordinator_id: u16,
    coordinator_signature: Vec<u8>,
    psbt_bytes: Vec<u8>,
    destination_address: String,
    consensus_params: ConsensusParameters,
    member_reports: Vec<MemberUtxoReport>,
    consensus_utxos: Vec<ConsensusUtxoInfo>,
    excluded_utxos: Vec<ExcludedUtxoInfo>,
    consensus_stats: ConsensusStatistics,
    data_integrity_hash: [u8; 32],
    created_at: u64, // Unix timestamp in seconds since epoch
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

    fn try_from(value: btc_server_client::Utxo) -> Result<Self, Self::Error> {
        // TODO: Implement conversion logic here
        Ok(Utxo {})
    }
}
