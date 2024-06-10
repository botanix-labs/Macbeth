use clap::Parser;
use displaydoc::Display as DisplayDoc;
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};
use thiserror::Error;
use tokio::{fs::File, io::AsyncReadExt};
use url::Url;

#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Open config file: {0}
    OpenConfig(std::io::Error),
    /// Failed to parse config: {0}
    ParseConfig(toml::de::Error),
    /// Failed to parse config as utf-8: {0}
    ParseUtf8(std::string::FromUtf8Error),
    /// Failed to read config file: {0}
    ReadConfig(std::io::Error),
    /// Failed to read config metadata: {0}
    ReadMeta(std::io::Error),
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct GrpcConfig {
    /// whether to enable gRPC reflection
    pub enable_reflection: bool,
    /// which compression encodings does the server accept for requests
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accept_compressed: Option<String>,
    /// which compression encodings might the server use for responses
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_compressed: Option<String>,
    /// limits the maximum size of a decoded message. Defaults to 4MB
    pub max_decoding_message_size: usize,
    /// limits the maximum size of an encoded message. Defaults to 4MB
    pub max_encoding_message_size: usize,
    /// limits the maximum size of streaming channel
    pub max_channel_size: usize,
    /// set a timeout on for all request handlers
    #[serde(deserialize_with = "deserialize_duration_from_usize")]
    pub timeout: Duration,
    /// sets the SETTINGS_INITIAL_WINDOW_SIZE spec option for HTTP2 stream-level flow control.
    /// Default is 65,535
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_stream_window_size: Option<u32>,
    /// set whether TCP keepalive messages are enabled on accepted connections
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tcp_keepalive: Option<Duration>,
    /// sets the max connection-level flow control for HTTP2. Default is 65,535
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_connection_window_size: Option<u32>,
    /// sets the maximum frame size to use for HTTP2. If not set, will default from underlying
    /// transport
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_frame_size: Option<u32>,
    /// set the concurrency limit applied to on requests inbound per connection. Defaults to 32
    pub concurrency_limit_per_connection: usize,
    /// sets the SETTINGS_MAX_CONCURRENT_STREAMS spec option for HTTP2 connections. Default is no
    /// limit (`None`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_streams: Option<u32>,
    /// set whether HTTP2 Ping frames are enabled on accepted connections. Default is no HTTP2
    /// keepalive (`None`)
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_duration_option"
    )]
    pub http2_keepalive_interval: Option<Duration>,
    /// sets a timeout for receiving an acknowledgement of the keepalive ping. Default is 20
    /// seconds
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_duration_option"
    )]
    pub http2_keepalive_timeout: Option<Duration>,
    /// sets whether to use an adaptive flow control. Defaults to false
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http2_adaptive_window: Option<bool>,
    /// set the value of `TCP_NODELAY` option for accepted connections. Enabled by default
    pub tcp_nodelay: bool,
    /// when looking for next draw we want to look at max `draw_lookahead_period_count`
    pub draw_lookahead_period_count: u64,
}

fn deserialize_duration_from_usize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    let seconds = u64::deserialize(deserializer)?;
    Ok(Duration::from_secs(seconds))
}

fn deserialize_duration_option<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
where
    D: Deserializer<'de>,
{
    let seconds: Option<u64> = Option::deserialize(deserializer)?;
    if seconds.is_none() {
        return Ok(None);
    }
    Ok(seconds.map(Duration::from_secs))
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TomlConfig {
    pub grpc: GrpcConfig,
}

impl TomlConfig {
    pub async fn new(path: impl AsRef<Path> + Send) -> Result<Self, Error> {
        read_to_string(path).await?.parse()
    }
}

impl FromStr for TomlConfig {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        toml::from_str(s).map_err(Error::ParseConfig)
    }
}

async fn read_to_string(path: impl AsRef<Path> + Send) -> Result<String, Error> {
    let mut file = File::open(path).await.map_err(Error::OpenConfig)?;
    let meta = file.metadata().await.map_err(Error::ReadMeta)?;
    let mut contents = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.read_to_end(&mut contents).await.map_err(Error::ReadConfig)?;
    String::from_utf8(contents).map_err(Error::ParseUtf8)
}

// Cli args and config

#[derive(Clone, Debug, Parser, Default, Deserialize, Serialize)]
pub(crate) struct CliConfig {
    /// The path to the database.
    #[arg(long)]
    db: Option<PathBuf>,
    /// The path to the database.
    #[arg(long)]
    config_path: Option<PathBuf>,
    /// The bitcoin network to operate on.
    #[arg(long)]
    btc_network: Option<bitcoin::Network>,
    /// Frost participant identifier
    #[arg(long)]
    identifier: Option<u16>,
    #[arg(long)]
    address: Option<String>,
    /// max signers
    #[arg(long)]
    max_signers: Option<u16>,
    /// min signers
    #[arg(long)]
    min_signers: Option<u16>,
    /// toml configuration path
    #[arg(long)]
    toml: Option<PathBuf>,
    /// jwt secret path
    #[arg(long)]
    jwt_secret: Option<PathBuf>,
    #[arg(long)]
    /// bitcoind url
    bitcoind_url: Option<Url>,
    #[arg(long)]
    /// bitcoind user
    bitcoind_user: Option<String>,
    #[arg(long)]
    /// bitcoind pass
    bitcoind_pass: Option<String>,
    #[arg(long)]
    /// acceptable fee rate difference percentage as an integer (ex. 2 = 2%, 20 = 20%)
    pub fee_rate_diff_percentage: Option<u32>,
    /// Fall back fee rate expressed in sat per vbyte
    #[arg(long)]
    fall_back_fee_rate_sat_per_vbyte: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct Config {
    /// The path to the database.
    pub(crate) db: PathBuf,
    /// The bitcoin network to operate on.
    pub(crate) btc_network: bitcoin::Network,
    /// Frost participant identifier
    pub(crate) identifier: u16,
    pub(crate) address: String,
    /// max signers
    pub(crate) max_signers: u16,
    /// min signers
    pub(crate) min_signers: u16,
    /// toml configuration path
    pub(crate) toml: Option<PathBuf>,
    /// jwt secret path
    pub(crate) jwt_secret: Option<PathBuf>,
    /// bitcoind url
    pub(crate) bitcoind_url: Url,
    /// bitcoind user
    pub(crate) bitcoind_user: String,
    /// bitcoind pass
    pub(crate) bitcoind_pass: String,
    /// acceptable fee rate difference percentage as an integer (ex. 2 = 2%, 20 = 20%)
    pub fee_rate_diff_percentage: u32,
    /// Fall back fee rate expressed in sat per vbyte
    pub(crate) fall_back_fee_rate_sat_per_vbyte: u64,
}

pub fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    // First parse from cli
    let cli_config = CliConfig::parse();
    // Initialize settings from file if specified
    let mut file_config = CliConfig::default();
    if let Some(path) = &cli_config.config_path {
        file_config = confy::load_path::<CliConfig>(&path).unwrap();
        info!("Loaded config from file: {:?}", path);
    }

    let config = Config {
        db: cli_config.db.or(file_config.db).expect("db is required"),
        toml: cli_config.toml.or(file_config.toml),
        btc_network: cli_config
            .btc_network
            .or(file_config.btc_network)
            .expect("btc_network is required"),
        identifier: cli_config
            .identifier
            .or(file_config.identifier)
            .expect("identifier is required"),
        address: cli_config.address.or(file_config.address).expect("address is required"),
        max_signers: cli_config
            .max_signers
            .or(file_config.max_signers)
            .expect("max_signers is required"),
        min_signers: cli_config
            .min_signers
            .or(file_config.min_signers)
            .expect("min_signers is required"),
        jwt_secret: cli_config.jwt_secret.or(file_config.jwt_secret),
        bitcoind_url: cli_config
            .bitcoind_url
            .or(file_config.bitcoind_url)
            .expect("bitcoind_url is required"),
        bitcoind_user: cli_config
            .bitcoind_user
            .or(file_config.bitcoind_user)
            .expect("bitcoind_user is required"),
        bitcoind_pass: cli_config
            .bitcoind_pass
            .or(file_config.bitcoind_pass)
            .expect("bitcoind_pass is required"),
        fee_rate_diff_percentage: cli_config
            .fee_rate_diff_percentage
            .or(file_config.fee_rate_diff_percentage)
            .expect("fee_rate_diff_percentage is required"),
        fall_back_fee_rate_sat_per_vbyte: cli_config
            .fall_back_fee_rate_sat_per_vbyte
            .or(file_config.fall_back_fee_rate_sat_per_vbyte)
            .expect("fall_back_fee_rate_sat_per_vbyte is required"),
    };

    Ok(config)
}
