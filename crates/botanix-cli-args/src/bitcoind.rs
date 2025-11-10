use botanix_cli_parsers::parsers::parse_url;
use clap::Args;
use serde::{Deserialize, Serialize};
use url::Url;

/// Default bitcoind url
pub const DEFAULT_BITCOIND_URL: &str = "localhost:18443";

/// Default bitcoind username
pub const DEFAULT_BITCOIND_USERNAME: &str = "foo";

/// Default bitcoind password
pub const DEFAULT_BITCOIND_PASSWORD: &str = "bar";

/// Parameters to configure Bitcoind.
#[derive(Debug, Clone, Args, PartialEq, Eq, Serialize, Deserialize)]
#[clap(next_help_heading = "Bitcoind")]
pub struct BitcoindArgs {
    /// bitcoind RPC primary url
    ///
    /// The primary url of the bitcoind server.
    #[arg(default_value_t=Url::parse(DEFAULT_BITCOIND_URL).expect("valid url"), value_parser = parse_url, long = "bitcoind.primary_url", name = "bitcoind.url", value_name = "BITCOIND_URL", env = "RETH_BITCOIND_URL"
    )]
    pub primary_url: Url,

    /// Bitcoind primary username
    ///
    /// The primary username of the bitcoind server.
    #[arg(
        default_value_t = DEFAULT_BITCOIND_USERNAME.into(),
        long = "bitcoind.primary_username",
        name = "bitcoind.primary_username",
        value_name = "BITCOIND_PRIMARY_USERNAME",
        env = "RETH_BITCOIND_PRIMARY_USERNAME"
    )]
    pub primary_username: String,

    /// Btcd primary password
    ///
    /// The primary password of the bitcoind server.
    #[arg(
        default_value_t = DEFAULT_BITCOIND_PASSWORD.into(),
        long = "bitcoind.primary_password",
        name = "bitcoind.primary_password",
        value_name = "BITCOIND_PRIMARY_PASSWORD",
        env = "RETH_BITCOIND_PRIMARY_PASSWORD"
    )]
    pub primary_password: String,

    /// bitcoind RPC secondary url
    ///
    /// The secondary url of the bitcoind server.
    #[arg(default_value_t=Url::parse(DEFAULT_BITCOIND_URL).expect("valid url"), value_parser = parse_url, long = "bitcoind.secondary_url", name = "bitcoind.url", value_name = "BITCOIND_URL", env = "RETH_BITCOIND_URL"
    )]
    pub secondary_url: Url,

    /// Btcd secondary username
    ///
    /// The secondary username of the bitcoind server.
    #[arg(
        default_value_t = DEFAULT_BITCOIND_USERNAME.into(),
        long = "bitcoind.secondary_username",
        name = "bitcoind.secondary_username",
        value_name = "BITCOIND_SECONDARY_USERNAME",
        env = "RETH_BITCOIND_SECONDARY_USERNAME"
    )]
    pub secondary_username: String,

    /// Btcd secondary password
    ///
    /// The secondary password of the bitcoind server.
    #[arg(
        default_value_t = DEFAULT_BITCOIND_PASSWORD.into(),
        long = "bitcoind.secondary_password",
        name = "bitcoind.secondary_password",
        value_name = "BITCOIND_SECONDARY_PASSWORD",
        env = "RETH_BITCOIND_SECONDARY_PASSWORD"
    )]
    pub secondary_password: String,

    /// ZMQ address for bitcoind
    #[arg(
        long = "bitcoind.zmq.hash-block-address",
        name = "bitcoind.zmq.hash-block-address",
        value_name = "BITCOIND_ZMQ_HASH_BLOCK_ADDRESS",
        env = "BITCOIND_ZMQ_HASH_BLOCK_ADDRESS"
    )]
    pub zmq_hash_block_address: Option<Url>,
}

impl Default for BitcoindArgs {
    fn default() -> Self {
        Self {
            primary_url: Url::parse(DEFAULT_BITCOIND_URL).expect("valid url"),
            primary_username: DEFAULT_BITCOIND_USERNAME.into(),
            primary_password: DEFAULT_BITCOIND_PASSWORD.into(),
            secondary_url: Url::parse(DEFAULT_BITCOIND_URL).expect("valid url"),
            secondary_username: DEFAULT_BITCOIND_USERNAME.into(),
            secondary_password: DEFAULT_BITCOIND_PASSWORD.into(),
            zmq_hash_block_address: None,
        }
    }
}
