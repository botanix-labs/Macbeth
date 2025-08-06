use reth_codecs::Compact;
use serde::{Deserialize, Serialize};

// TODO: TBD

pub type WalletSweepSessionId = [u8; 32];

#[derive(Debug, Clone, Serialize, Deserialize, Compact)]
pub struct WalletSweepSession {
    psbt_bytes: Vec<u8>,
    destination_address: String,
    consensus_params: ConsensusParameters,
    created_at: u64, // Unix timestamp in seconds since epoch
}

impl WalletSweepSession {
    pub fn calculate_id(&self) -> WalletSweepSessionId {
        todo!()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Compact)]
pub struct ConsensusParameters {
    fee_rate_sat_vb: u64,
    utxo_ordering: String, // "lexicographic"
    threshold_percent: u8,
    reachable_members: Vec<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Compact)]
pub struct Utxo {}

impl TryFrom<btc_server_client::Utxo> for Utxo {
    type Error = eyre::Error;

    fn try_from(value: btc_server_client::Utxo) -> Result<Self, Self::Error> {
        todo!()
    }
}
