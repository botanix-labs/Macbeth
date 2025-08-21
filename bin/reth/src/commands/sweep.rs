//! Emergency wallet sweep command implementation

use bitcoin::{address::NetworkUnchecked, FeeRate, hashes::{sha256, Hash, hex::FromHex}};
use botanix_wallet_sweep::{create_psbt_async, request::DestinationConfig, WalletSweepRequest};
use btc_server_client::{jwt::JwtSecret, BtcServerExtendedApi, Empty, GrpcClientFactory};
use botanix_configs::federation::FederationTomlConfig;
use clap::{Parser, Subcommand};
use eyre::WrapErr;
use reth_cli_runner::CliContext;

use std::{fs, net::SocketAddr, path::PathBuf, str::FromStr};
use tracing::{info, warn};

/// Emergency wallet sweep operations for the Botanix federation
#[derive(Debug, Parser)]
pub struct SweepCommand {
    /// Local BTC server address
    #[arg(long, default_value = "127.0.0.1:8080", value_parser = clap::value_parser!(SocketAddr))]
    btc_server_address: SocketAddr,

    #[arg(long, value_parser = parse_file_exists)]
    btc_server_jwt_secret_path: PathBuf,

    /// Emergency sweep subcommand to execute
    #[command(subcommand)]
    pub command: SweepSubcommands,
}

/// Available emergency sweep subcommands
#[derive(Debug, Subcommand)]
pub enum SweepSubcommands {
    /// Initiate an emergency sweep session (coordinator only)
    ///
    /// This command initiates a wallet sweep session by creating a request
    /// that should be share with other federation members to accept siging session.
    #[command()]
    Initiate {
        /// Destination address, network, and fee rate configuration
        #[command(flatten)]
        destination: DestinationOptions,
        /// Path to federation config path
        #[arg(long, value_parser = parse_file_exists)]
        federation_config_path: PathBuf,
        /// Path to coordinator private key
        #[arg(long, value_parser = parse_file_exists)]
        coordinator_key: PathBuf,
        // /// JWT secret file path for btc-server authentication
        // #[arg(long)]
        // jwt_secret: Option<PathBuf>,
        // /// Timeout in seconds for member queries
        // #[arg(long, default_value = "30")]
        // timeout: u64,
        // /// Chunk size for UTXO pagination
        // #[arg(long, default_value = "1000")]
        // chunk_size: u32,
        /// Path where the wallet sweep request JSON file will be saved
        #[arg(long, value_parser = parse_file_not_exists)]
        output_request_file_path: PathBuf,
    },
    /// Accept an emergency sweep request to participate in signing session
    #[command()]
    AcceptRequest {
        /// Path to wallet sweep request JSON file
        #[arg(long, value_parser = parse_file_exists)]
        request_file_path: PathBuf,
    },
    /// Construct PSBT from an emergency sweep request for ingesting or manual signing
    #[command()]
    Psbt {
        /// Path to wallet sweep request JSON file
        #[arg(long, value_parser = parse_file_exists)]
        request_file_path: PathBuf,
    },
}

/// Destination configuration options for wallet sweep operations
#[derive(Debug, Parser)]
pub struct DestinationOptions {
    /// Bitcoin network to use (mainnet, testnet, regtest)
    #[arg(long)]
    network: bitcoin::Network,
    /// Destination address for swept funds
    #[arg(long)]
    address: bitcoin::Address<NetworkUnchecked>,
    /// Fee rate in sat/vB
    #[arg(long, value_parser = parse_fee_rate)]
    fee_rate: FeeRate,
}

impl DestinationConfig for DestinationOptions {
    fn network(&self) -> eyre::Result<bitcoin::Network> {
        Ok(self.network)
    }

    fn address(&self) -> eyre::Result<bitcoin::Address> {
        let address = self.address.clone().require_network(self.network).wrap_err_with(|| {
            format!(
                "invalid destination address: {}",
                self.address.clone().assume_checked().to_string()
            )
        })?;

        Ok(address)
    }

    fn fee_rate(&self) -> eyre::Result<FeeRate> {
        Ok(self.fee_rate)
    }
}

impl SweepCommand {
    /// Validate if we are the coordinator by checking our federation configuration and private key
    async fn validate_coordinator_authority(
        request: &WalletSweepRequest,
        btc_server_client: &mut impl BtcServerExtendedApi,
    ) -> eyre::Result<bool> {
        // Get our local identifier from btc-server configuration
        let public_key_response = btc_server_client.get_public_key(Empty {}).await
            .wrap_err("Failed to get our public key from btc-server")?;
        
        // Parse our public key using Vec<u8> to avoid sizing issues
        let our_public_key_bytes: Vec<u8> = FromHex::from_hex(&public_key_response.publickey)
            .wrap_err("Failed to decode our public key")?;
        let our_public_key = bitcoin::secp256k1::PublicKey::from_slice(&our_public_key_bytes)
            .wrap_err("Failed to parse our public key")?;
        
        // Verify the coordinator signature to check if we are the coordinator
        let secp = bitcoin::secp256k1::Secp256k1::new();
        
        // Reconstruct the signature data that was signed
        let mut signature_data = Vec::new();
        signature_data.extend_from_slice(&request.coordinator_id.to_le_bytes());
        signature_data.extend_from_slice(&bitcoin::Network::from_str(&request.destination_network)?.magic().to_bytes());
        signature_data.extend_from_slice(request.destination_address.clone().assume_checked().to_string().as_bytes());
        signature_data.extend_from_slice(&request.fee_rate_sat_vb.to_le_bytes());
        signature_data.extend_from_slice(&request.created_at.to_le_bytes());
        
        // Create message hash
        let hash = sha256::Hash::hash(&signature_data);
        let message = bitcoin::secp256k1::Message::from_digest_slice(hash.as_ref())
            .wrap_err("Failed to create message for signature verification")?;
        
        // Parse the coordinator signature
        let signature = bitcoin::secp256k1::ecdsa::Signature::from_compact(&request.coordinator_signature)
            .wrap_err("Failed to parse coordinator signature")?;
        
        // Verify if the signature was created by our private key
        match secp.verify_ecdsa(&message, &signature, &our_public_key) {
            Ok(()) => {
                info!("Coordinator authority validated - we are the coordinator");
                Ok(true)
            }
            Err(_) => {
                info!("We are not the coordinator for this wallet sweep request");
                Ok(false)
            }
        }
    }

    /// Execute the sweep command
    pub async fn execute(&self, _ctx: CliContext) -> eyre::Result<()> {
        info!("Starting emergency sweep command");

        let btc_server_jwt_secret = JwtSecret::from_file(&self.btc_server_jwt_secret_path)
            .wrap_err_with(|| {
                format!(
                    "Failed to read btc server jwt token from {:?}",
                    self.btc_server_jwt_secret_path
                )
            })?;

        let btc_server_factory = GrpcClientFactory::new(
            self.btc_server_address.to_string(),
            Some(btc_server_jwt_secret),
        );

        let mut btc_server_client =
            btc_server_factory.build_and_connect().await.wrap_err_with(|| {
                format!(
                    "Failed to connect to btc server at {} with JWT secret {}",
                    self.btc_server_address,
                    self.btc_server_jwt_secret_path.display()
                )
            })?;

        info!("Btc server connected");

        // Check our connection to the btc server is authenticated properly
        btc_server_client
            .health_check(Empty {})
            .await
            .map_err(|err| eyre::eyre!("Failed to authenticate to btc server: {}", err))?;

        info!("Btc server authenticated");

        match &self.command {
            SweepSubcommands::Initiate {
                destination,
                federation_config_path,
                coordinator_key,
                output_request_file_path,
            } => {
                let federation_config_string = std::fs::read_to_string(&federation_config_path)?;
                let federation_config = FederationTomlConfig::from_str(&federation_config_string)
                    .wrap_err("Invalid federation TOML config")?;

                info!("Starting emergency sweep initiation");
                info!(
                    "Destination: {}, Fee rate: {} sat/vB, Network: {}",
                    destination.address()?, 
                    destination.fee_rate()?.to_sat_per_vb_floor(),
                    destination.network()?
                );
                info!(
                    "Federation config: {}, Coordinator key: {}",
                    federation_config_path.display(), 
                    coordinator_key.display()
                );

                // Build the wallet sweep request with proper coordinator identification
                let session_request = WalletSweepRequest::build_with_federation_config(
                    destination,
                    coordinator_key,
                    &federation_config,
                )?;

                info!(
                    "Created wallet sweep request for coordinator ID: {}, notifying btc-server",
                    session_request.coordinator_id
                );
                
                // Accept the request on the local btc-server (this creates the session and notifies other members)
                session_request.accept(&mut btc_server_client).await?;

                info!("Wallet sweep session created and notification sent to federation members");

                // Serialize and save the request to file for distribution to other members
                let request_string = serde_json::to_string_pretty(&session_request)
                    .wrap_err_with(|| "Failed to serialize wallet sweep request")?;

                fs::write(output_request_file_path, &request_string)
                    .wrap_err_with(|| format!("Failed to write request to {:?}", output_request_file_path))?;
                
                info!("Wallet sweep request saved to: {:?}", output_request_file_path);
                info!("Distribute this file to other federation members via secure channels");
            }
            SweepSubcommands::AcceptRequest { request_file_path } => {
                info!("Processing wallet sweep accept request from file: {:?}", request_file_path);
                
                // Load and validate the wallet sweep request
                let request = WalletSweepRequest::from_json_file(request_file_path).await
                    .wrap_err_with(|| format!("Failed to load wallet sweep request from {:?}", request_file_path))?;
                
                info!(
                    "Loaded wallet sweep request - Coordinator ID: {}, Destination: {}, Fee rate: {} sat/vB",
                    request.coordinator_id,
                    request.destination_address.clone().assume_checked(),
                    request.fee_rate_sat_vb
                );

                // Accept the wallet sweep session (creates session in btc-server)
                request.accept(&mut btc_server_client).await
                    .wrap_err("Failed to accept wallet sweep session")?;
                
                info!("Wallet sweep session accepted and stored in btc-server");

                // Check if we are the coordinator by comparing the coordinator signature
                // We need to validate if this request was signed by our private key
                let is_coordinator = Self::validate_coordinator_authority(&request, &mut btc_server_client)
                    .await?;
                
                if is_coordinator {
                    info!("We are the coordinator - wallet sweep session accepted");
                    info!("FROST task will automatically detect the session and create sweep PSBT");
                    info!("FROST signing process will coordinate with federation members using SigningPsbtType::Sweep");
                    info!("Federation members will validate the sweep PSBT against their local UTXO sets");
                    info!("The sweep transaction will be automatically broadcast when threshold signatures are collected");
                } else {
                    info!("We are a federation member (not coordinator) - wallet sweep session accepted");
                    info!("Ready for signing validation when coordinator initiates FROST signing process");
                    info!("Will validate coordinator's sweep PSBT against local UTXO set during signing");
                }
            }
            SweepSubcommands::Psbt { request_file_path } => {
                let request = WalletSweepRequest::from_json_file(request_file_path).await?;

                let psbt = create_psbt_async(request, &mut btc_server_client).await.wrap_err_with(
                    || format!("Failed to create PSBT from request file {:?}", request_file_path),
                )?;

                // Print PSBT in base64 format for use with other tools
                use bitcoin::base64::{engine::general_purpose, Engine as _};
                let psbt_base64 = general_purpose::STANDARD.encode(&psbt.serialize());
                println!("Emergency sweep PSBT:");
                println!("{}", psbt_base64);

                // Save to file with overwrite protection
                let psbt_filename = request_file_path.with_extension("psbt");
                if psbt_filename.exists() {
                    warn!("PSBT file {:?} already exists, overwriting...", psbt_filename);
                }

                fs::write(&psbt_filename, psbt_base64.as_bytes()).wrap_err_with(|| {
                    format!("Failed to write PSBT to file {:?}", psbt_filename)
                })?;
                info!("PSBT saved to: {:?}", psbt_filename);
            }
        }

        Ok(())
    }
}

fn parse_file_exists(path: &str) -> Result<PathBuf, String> {
    let path_buf = PathBuf::from(path);
    if path_buf.exists() && path_buf.is_file() {
        Ok(path_buf)
    } else {
        Err(format!("File '{}' does not exist", path))
    }
}

fn parse_file_not_exists(path: &str) -> Result<PathBuf, String> {
    let path_buf = PathBuf::from(path);
    if !path_buf.exists() {
        Ok(path_buf)
    } else {
        Err(format!("File '{}' already exists", path))
    }
}

fn parse_fee_rate(rate: &str) -> Result<FeeRate, String> {
    let sat_vb = rate.parse::<u64>().map_err(|_| format!("Invalid fee rate: {}", rate))?;
    if sat_vb == 0 {
        return Err("Fee rate cannot be zero".to_string());
    }

    FeeRate::from_sat_per_vb(sat_vb).ok_or(format!("Too big fee rate {}", rate))
}
