use crate::encoding::PARSER;
use bitcoin::{address::NetworkUnchecked, Address, Network};
use bitcoincore_rpc::jsonrpc::serde_json;
use botanix_storage::models::WalletSweepSession;
use btc_server_client::{
    AcceptWalletSweepSessionRequest, BtcServerExtendedApi, BtcServerExtendedClient,
};
use eyre::WrapErr;
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, fs, path::Path, str::FromStr, time::SystemTime};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletSweepRequest {
    pub coordinator_id: u16,
    pub coordinator_signature: Vec<u8>,
    pub destination_network: String,
    pub destination_address: Address<NetworkUnchecked>,
    pub fee_rate_sat_vb: u64,
    pub created_at: u64, // Unix timestamp in seconds since epoch
}

pub trait DestinationConfig: Debug {
    fn network(&self) -> eyre::Result<bitcoin::Network>;
    fn address(&self) -> eyre::Result<bitcoin::Address>;

    fn fee_rate(&self) -> eyre::Result<bitcoin::FeeRate>;
}

impl WalletSweepRequest {
    pub fn build() -> eyre::Result<Self> {
        // TODO: Accept all needed params and create request

        let created_at = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

        Ok(Self {
            coordinator_id: 0,
            coordinator_signature: vec![],
            destination_network: Network::Bitcoin.to_string(),
            destination_address: Address::from_str("bc1qexampleaddress1234567890abcdefg")?,
            fee_rate_sat_vb: 0,
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

impl TryInto<WalletSweepSession> for WalletSweepRequest {
    type Error = eyre::Error;

    fn try_into(self) -> Result<WalletSweepSession, Self::Error> {
        // TODO: Implement conversion logic here
        let session = WalletSweepSession {
            bitcoin_network: self.destination_network.parse()?,
            bitcoin_destination_address: self.destination_address,
            created_at: self.created_at,
        };

        Ok(session)
    }
}
