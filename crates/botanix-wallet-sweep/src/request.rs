use crate::encoding::PARSER;
use bitcoin::{address::NetworkUnchecked, Address, Network, secp256k1, hashes::{sha256, Hash}};
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
    /// Determine coordinator ID from federation config and private key
    fn determine_coordinator_id(
        coordinator_key_path: &Path,
        federation_config: &botanix_configs::federation::FederationTomlConfig,
    ) -> eyre::Result<u16> {
        // Read and parse the coordinator private key
        let coordinator_key_data = fs::read_to_string(coordinator_key_path)
            .wrap_err_with(|| format!("Failed to read coordinator key from {:?}", coordinator_key_path))?;
        
        let sanitized_key = coordinator_key_data.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>();
        let coordinator_secret_key = sanitized_key
            .as_str()
            .parse::<secp256k1::SecretKey>()
            .wrap_err("Invalid coordinator private key format")?;
        
        // Derive the public key from the private key
        let secp = secp256k1::Secp256k1::new();
        let coordinator_public_key = coordinator_secret_key.public_key(&secp);
        
        // Find the coordinator's position in the federation config
        for (index, member) in federation_config.federation_member_public_key.iter().enumerate() {
            let member_public_key = secp256k1::PublicKey::from_str(&member.key)
                .wrap_err_with(|| format!("Invalid public key in federation config: {}", member.key))?;
            
            if member_public_key == coordinator_public_key {
                return Ok(index as u16);
            }
        }
        
        Err(eyre::eyre!("Coordinator private key does not match any federation member"))
    }
    
    /// Build a new wallet sweep request with the provided parameters
    pub fn build_with_federation_config(
        destination: &impl DestinationConfig,
        coordinator_key_path: &Path,
        federation_config: &botanix_configs::federation::FederationTomlConfig,
    ) -> eyre::Result<Self> {
        let created_at = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
        
        // Determine coordinator ID from the federation config and private key
        let coordinator_id = Self::determine_coordinator_id(coordinator_key_path, federation_config)?;
        
        // Read coordinator private key
        let coordinator_key_data = fs::read_to_string(coordinator_key_path)
            .wrap_err_with(|| format!("Failed to read coordinator key from {:?}", coordinator_key_path))?;
        
        let sanitized_key = coordinator_key_data.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>();
        let coordinator_secret_key = sanitized_key
            .as_str()
            .parse::<secp256k1::SecretKey>()
            .wrap_err("Invalid coordinator private key format")?;
        
        // Create signature data (sign the request context)
        let network = destination.network()?;
        let address = destination.address()?;
        let fee_rate = destination.fee_rate()?;
        
        let mut signature_data = Vec::new();
        signature_data.extend_from_slice(&coordinator_id.to_le_bytes());
        signature_data.extend_from_slice(&network.magic().to_bytes());
        signature_data.extend_from_slice(address.to_string().as_bytes());
        signature_data.extend_from_slice(&fee_rate.to_sat_per_vb_floor().to_le_bytes());
        signature_data.extend_from_slice(&created_at.to_le_bytes());
        
        // Create signature using secp256k1
        let secp = secp256k1::Secp256k1::new();
        let hash = sha256::Hash::hash(&signature_data);
        let message = secp256k1::Message::from_digest_slice(hash.as_ref())
            .wrap_err("Failed to create message for signing")?;
        let signature = secp.sign_ecdsa(&message, &coordinator_secret_key);
        let coordinator_signature = signature.serialize_compact().to_vec();

        Ok(Self {
            coordinator_id,
            coordinator_signature,
            destination_network: network.to_string(),
            destination_address: address.as_unchecked().clone(),
            fee_rate_sat_vb: fee_rate.to_sat_per_vb_floor(),
            created_at,
        })
    }

    /// Build a new wallet sweep request with the provided parameters (legacy method)
    pub fn build(
        coordinator_id: u16,
        destination: &impl DestinationConfig,
        coordinator_key_path: &Path,
    ) -> eyre::Result<Self> {
        let created_at = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
        
        // Read coordinator private key
        let coordinator_key_data = fs::read_to_string(coordinator_key_path)
            .wrap_err_with(|| format!("Failed to read coordinator key from {:?}", coordinator_key_path))?;
        
        let sanitized_key = coordinator_key_data.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>();
        let coordinator_secret_key = sanitized_key
            .as_str()
            .parse::<secp256k1::SecretKey>()
            .wrap_err("Invalid coordinator private key format")?;
        
        // Create signature data (sign the request context)
        let network = destination.network()?;
        let address = destination.address()?;
        let fee_rate = destination.fee_rate()?;
        
        let mut signature_data = Vec::new();
        signature_data.extend_from_slice(&coordinator_id.to_le_bytes());
        signature_data.extend_from_slice(&network.magic().to_bytes());
        signature_data.extend_from_slice(address.to_string().as_bytes());
        signature_data.extend_from_slice(&fee_rate.to_sat_per_vb_floor().to_le_bytes());
        signature_data.extend_from_slice(&created_at.to_le_bytes());
        
        // Create signature using secp256k1
        let secp = secp256k1::Secp256k1::new();
        let hash = sha256::Hash::hash(&signature_data);
        let message = secp256k1::Message::from_digest_slice(hash.as_ref())
            .wrap_err("Failed to create message for signing")?;
        let signature = secp.sign_ecdsa(&message, &coordinator_secret_key);
        let coordinator_signature = signature.serialize_compact().to_vec();

        Ok(Self {
            coordinator_id,
            coordinator_signature,
            destination_network: network.to_string(),
            destination_address: address.as_unchecked().clone(),
            fee_rate_sat_vb: fee_rate.to_sat_per_vb_floor(),
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
        let bitcoin_network: Network = self.destination_network.parse()
            .wrap_err("Invalid destination network")?;
        
        let session = WalletSweepSession {
            bitcoin_network,
            bitcoin_destination_address: self.destination_address,
            fee_rate_sat_vb: self.fee_rate_sat_vb,
            created_at: self.created_at,
        };

        Ok(session)
    }
}
