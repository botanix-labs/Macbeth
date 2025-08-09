//! Emergency wallet sweep command implementation

use bitcoin::{address::NetworkUnchecked, FeeRate};
use botanix_data_parser::{DataParser, SerializationType, DEFAULT_COMPRESSION_STRATEGY};
use botanix_wallet_sweep::{request::DestinationConfig, WalletSweepRequest};
use btc_server_client::{jwt::JwtSecret, BtcServerExtendedApi, Empty, GrpcClientFactory};
use btcserverlib::{database::Db, dkg, federation_args::FederationTomlConfig};
use clap::{Parser, Subcommand};
use eyre::{OptionExt, WrapErr};
use reth_cli_runner::CliContext;
use std::{fs, net::SocketAddr, path::PathBuf, str::FromStr};
use tracing::{error, info, warn};

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

#[derive(Debug, Parser)]
struct DestinationOptions {
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
    /// Execute the sweep command
    pub async fn execute(&self, _ctx: CliContext) -> eyre::Result<()> {
        info!("Starting emergency sweep command");

        let btc_server_jwt_secret = JwtSecret::from_file(&self.btc_server_jwt_secret_path)
            .wrap_err_with(|| {
                format!(
                    "Failed to read btc server jwt toke from {:?}",
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
                    .map_err(|_| {
                    dkg::Error::BadConfig("invalid federation Toml config".to_string())
                })?;

                info!("Starting emergency sweep initiation");
                // info!(
                //     "Destination: {}, Fee rate: {} sat/vB, Consensus threshold: {}%",
                //     destination, fee_rate, consensus_threshold
                // );
                // info!(target: "reth::cli", "Federation config: {}, Coordinator key: {}",
                //           federation_config.display(), coordinator_key.display());
                // info!(target: "reth::cli", "Timeout: {}s, Chunk size: {}", timeout, chunk_size);
                //
                // if let Some(jwt_path) = jwt_secret {
                //     info!(target: "reth::cli", "JWT secret: {}", jwt_path.display());
                // }

                let session_request = WalletSweepRequest::build()?;

                session_request.accept(&mut btc_server_client).await?;

                let request_string = serde_json::to_string(&session_request)
                    .wrap_err_with(|| "Failed to serialize wallet sweep request")?;

                fs::write(output_request_file_path, &request_string)?;
            }
            SweepSubcommands::AcceptRequest { request_file_path } => {
                let request = WalletSweepRequest::from_json_file(request_file_path).await?;

                request.accept(&mut btc_server_client).await?;
            }
            SweepSubcommands::Psbt { request_file_path } => {
                let request = WalletSweepRequest::from_json_file(request_file_path).await?;

                let psbt = botanix_wallet_sweep::create_psbt(request).wrap_err_with(|| {
                    format!("Failed to create PSBT from request file {:?}", request_file_path)
                })?;

                // TODO: Save PSBT to file or write to std out if pipe is provided
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
