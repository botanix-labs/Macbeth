//! Emergency wallet sweep command implementation

use bitcoin::FeeRate;
use botanix_btc_wallet::{
    dump::UtxoDumpsReader, dump_utxos_to_file, init::DestinationConfig, init_wallet_sweep,
};
use botanix_data_parser::{DataParser, SerializationType, DEFAULT_COMPRESSION_STRATEGY};
use btc_server_client::Empty;
use btcserverlib::{
    database::Db,
    dkg,
    extended_client::{BtcServerExtendedApi, GrpcClientFactory},
    federation_args::FederationTomlConfig,
    jwt::JwtSecret,
};
use clap::{Parser, Subcommand};
use eyre::{OptionExt, WrapErr};
use reth_cli_runner::CliContext;
use std::{net::SocketAddr, path::PathBuf, str::FromStr};
use tracing::{error, info, warn};

/// Emergency wallet sweep operations for the Botanix federation
#[derive(Debug, Parser)]
pub struct SweepCommand {
    /// Local BTC server address
    #[arg(long, default_value = "127.0.0.1:8080", value_parser = clap::value_parser!(SocketAddr))]
    btc_server_address: SocketAddr,

    #[arg(long, value_parser = clap::value_parser!(PathBuf).exists().is_file())]
    btc_server_jwt_secret_path: PathBuf,

    /// Emergency sweep subcommand to execute
    #[command(subcommand)]
    pub command: SweepSubcommands,
}

/// Available emergency sweep subcommands
#[derive(Debug, Subcommand)]
pub enum SweepSubcommands {
    #[command(about = "Dump wallet UTXOs to file")]
    DumpUtxos {
        /// Path to output file for UTXOs
        #[arg(long)]
        output_file_path: PathBuf,
    },
    /// Initiate emergency sweep coordination (coordinator only)
    #[command(about = "Initiate emergency sweep as designated coordinator")]
    Initiate {
        #[command(flatten)]
        destination: DestinationOptions,
        #[command(flatten)]
        utxo: UtxoOptions,
        /// Path to federation config path
        #[arg(long)]
        federation_config_path: PathBuf,
        /// Path to coordinator private key
        #[arg(long)]
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
    },
    /// Accept and validate emergency sweep request
    #[command(about = "Accept emergency sweep request from coordinator")]
    AcceptRequest {
        /// Path to sweep request JSON file
        request_file: PathBuf,
    },
}

#[derive(Debug, Parser)]
struct DestinationOptions {
    /// Bitcoin network to use (mainnet, testnet, regtest)
    #[arg(long)]
    network: bitcoin::Network,
    /// Destination address for swept funds
    #[arg(long)]
    address: String,
    /// Fee rate in sat/vB
    #[arg(long, value_parser = FeeRate::from_sat_per_vb(clap::value_parser!(u64)))]
    fee_rate: FeeRate,
}

impl DestinationConfig for DestinationOptions {
    fn network(&self) -> eyre::Result<bitcoin::Network> {
        Ok(self.network)
    }

    fn address(&self) -> eyre::Result<bitcoin::Address> {
        let address = self
            .address
            .parse::<bitcoin::Address<_>>()
            .and_then(|a| a.require_network(self.network))
            .wrap_err_with(|| format!("invalid destination address: {}", self.address))?;

        Ok(address)
    }

    fn fee_rate(&self) -> eyre::Result<FeeRate> {
        Ok(self.fee_rate)
    }
}

#[derive(Debug, Parser)]
struct UtxoOptions {
    /// Consensus threshold percentage (75-95)
    #[arg(long, default_value = "80", value_parser = clap::value_parser!(u8).range(75..=95))]
    consensus_threshold: u8,
    /// Path to directory with UTXO data files
    #[arg(long, value_parser = clap::value_parser!(PathBuf).exists().is_dir())]
    utxo_data_dir_path: PathBuf,
}

impl SweepCommand {
    /// Execute the sweep command
    pub async fn execute(&self, _ctx: CliContext) -> eyre::Result<()> {
        info!("Starting emergency sweep command");

        let btc_server_jwt_secret = JwtSecret::from_file(&self.btc_server_jwt_secret_path)
            .wrap_err_with(|| {
                format!(
                    "Failed to read btc server jwt toke from {}",
                    self.btc_server_jwt_secret_path.to_str()
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

        // Initialize data parser for utxo dump
        let parser = DataParser::default()
            .with_compression_strategy(&DEFAULT_COMPRESSION_STRATEGY)
            .with_serialization_type(SerializationType::Postcard);

        match &self.command {
            SweepSubcommands::DumpUtxos { output_file_path: output_file_file } => {
                dump_utxos_to_file(&mut btc_server_client, &parser, output_file_file)
                    .await
                    .wrap_err(eyre::eyre!("Failed to dump UTXOs"))?;
            }
            SweepSubcommands::Initiate {
                destination,
                utxo,
                federation_config_path,
                coordinator_key,
            } => {
                let federation_config_string = std::fs::read_to_string(&federation_config_path)?;
                let federation_config = FederationTomlConfig::from_str(&federation_config_string)
                    .map_err(|_| {
                    dkg::Error::BadConfig("invalid federation Toml config".to_string())
                })?;

                let sweep_request =
                    init_wallet_sweep(&mut btc_server_client, parser.clone(), destination).await?;
            }
            SweepSubcommands::AcceptRequest { request_file, btc_server_addr, jwt_secret } => {
                self.execute_accept_request(&db_path, request_file, btc_server_addr, jwt_secret)
                    .await?;
            }
            SweepSubcommands::Sign { request_file, btc_server_addr, jwt_secret } => {
                self.execute_sign(&db_path, request_file, btc_server_addr, jwt_secret).await?;
            }
        }

        Ok(())
    }

    /// Execute accept request command (placeholder)
    async fn execute_accept_request(
        &self,
        _db_path: &PathBuf,
        request_file: &PathBuf,
        btc_server_addr: &Option<String>,
        jwt_secret: &Option<PathBuf>,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", "Accepting emergency sweep request from: {}", request_file.display());

        if let Some(addr) = btc_server_addr {
            info!(target: "reth::cli", "btc-server address: {}", addr);
        }
        if let Some(jwt_path) = jwt_secret {
            info!(target: "reth::cli", "JWT secret: {}", jwt_path.display());
        }

        warn!(target: "reth::cli", "Emergency sweep accept-request command not yet implemented");
        println!("⚠️  Emergency sweep accept-request is not yet implemented.");
        println!("📋 This command will validate and accept coordinator sweep requests.");
        println!("🔍 Will verify coordinator authority and consensus decisions.");
        println!("⚖️  Will reconstruct PSBT and perform byte-for-byte validation.");
        println!("🤝 Will automatically join FROST signing upon validation.");
        Ok(())
    }

    /// Execute sign command (placeholder)
    async fn execute_sign(
        &self,
        _db_path: &PathBuf,
        request_file: &PathBuf,
        btc_server_addr: &Option<String>,
        jwt_secret: &Option<PathBuf>,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", "Participating in emergency sweep signing for: {}", request_file.display());

        if let Some(addr) = btc_server_addr {
            info!(target: "reth::cli", "btc-server address: {}", addr);
        }
        if let Some(jwt_path) = jwt_secret {
            info!(target: "reth::cli", "JWT secret: {}", jwt_path.display());
        }

        warn!(target: "reth::cli", "Emergency sweep sign command not yet implemented");
        println!("⚠️  Emergency sweep signing is not yet implemented.");
        println!("📋 This command will participate in threshold signing for emergency sweeps.");
        println!("🔐 Will re-verify PSBT matches accepted request file.");
        println!("🤝 Will contribute to FROST threshold signature.");
        Ok(())
    }
}
