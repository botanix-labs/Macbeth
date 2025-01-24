use clap::Parser;
use confy::ConfyError;
use displaydoc::Display as DisplayDoc;
use serde::{Deserialize, Deserializer};
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
    /// Failed to read env config: {0}
    Confy(ConfyError),
    /// Missing config element: {0}
    MissingConfigElement(&'static str),
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
    #[allow(dead_code)]
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
    /// when looking for next draw we want to look at max `draw_lookahead_period_count`\
    #[allow(dead_code)]
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

#[derive(Clone, Debug, Parser)]
pub struct CliConfig {
    /// The path to the database.
    #[arg(long)]
    db: PathBuf,
    /// The bitcoin network to operate on.
    #[arg(long)]
    btc_network: bitcoin::Network,
    /// Frost participant identifier
    #[arg(long)]
    identifier: u16,
    #[arg(long)]
    address: String,
    /// max signers
    #[arg(long)]
    max_signers: u16,
    /// min signers
    #[arg(long)]
    min_signers: u16,
    /// toml configuration path
    #[arg(long)]
    toml: Option<PathBuf>,
    /// jwt secret path
    #[arg(long)]
    btc_signing_server_jwt_secret: Option<PathBuf>,
    #[arg(long)]
    /// bitcoind url
    bitcoind_url: Url,
    #[arg(long)]
    /// bitcoind user
    bitcoind_user: String,
    #[arg(long)]
    /// bitcoind pass
    bitcoind_pass: String,
    #[arg(long)]
    /// acceptable fee rate difference percentage as an integer (ex. 2 = 2%, 20 = 20%)
    fee_rate_diff_percentage: Option<u32>,
    /// Fall back fee rate expressed in sat per vbyte
    #[arg(long)]
    fall_back_fee_rate_sat_per_vbyte: Option<u64>,
    /// http port
    #[arg(long)]
    metrics_port: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// The path to the database.
    pub db: PathBuf,
    /// The bitcoin L1 network
    pub btc_network: bitcoin::Network,
    /// Frost participant identifier. Should be your index into the chain.toml federation pk list
    /// for example if you are the first signer in the chain.toml you should use 0
    pub identifier: u16,
    /// Address to bind to.
    pub address: String,
    /// multisig max signers
    pub max_signers: u16,
    /// multisig min signers
    pub min_signers: u16,
    /// toml configuration path. Leave blank to use defaults
    pub toml: Option<PathBuf>,
    /// jwt secret path
    pub btc_signing_server_jwt_secret: Option<PathBuf>,
    /// bitcoind url
    pub bitcoind_url: Url,
    /// bitcoind RPC user
    pub bitcoind_user: String,
    /// bitcoind RPC pass
    pub bitcoind_pass: String,
    /// metrics port
    pub metrics_port: u16,
    /// acceptable fee rate difference percentage as an integer (ex. 2 = 2%, 20 = 20%)
    /// signing will refuse to sign if the fee rate is more than this percentage off from the
    pub fee_rate_diff_percentage: u32,
    /// Fall back fee rate expressed in sat per vbyte
    pub fall_back_fee_rate_sat_per_vbyte: u64,
}

pub fn load_config() -> Result<Config, Error> {
    // First parse from cli
    let cli_config = CliConfig::parse();

    let config = Config {
        db: cli_config.db,
        toml: cli_config.toml,
        btc_network: cli_config.btc_network,
        identifier: cli_config.identifier,
        address: cli_config.address,
        max_signers: cli_config.max_signers,
        min_signers: cli_config.min_signers,
        btc_signing_server_jwt_secret: cli_config.btc_signing_server_jwt_secret,
        bitcoind_url: cli_config.bitcoind_url,
        bitcoind_user: cli_config.bitcoind_user,
        bitcoind_pass: cli_config.bitcoind_pass,
        metrics_port: cli_config.metrics_port.unwrap_or(7000),
        fee_rate_diff_percentage: cli_config.fee_rate_diff_percentage.unwrap_or(2),
        fall_back_fee_rate_sat_per_vbyte: cli_config.fall_back_fee_rate_sat_per_vbyte.unwrap_or(10),
    };
    Ok(config)
}
