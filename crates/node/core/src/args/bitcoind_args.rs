use clap::Args;
use reth_btc_wallet::bitcoind::BitcoindConfig;
use reth_cli_util::parsers::parse_url;
use serde::{Deserialize, Serialize};
use url::Url;

/// Default bitcoind url
pub(crate) const DEFAULT_BITCOIND_URL: &str = "localhost:18443";

/// Default bitcoind ZMQ address
pub(crate) const DEFAULT_BITCOIND_ZMQ_HASH_BLOCK_ADDRESS: &str = "tcp://localhost:28332";

/// Default bitcoind username
pub(crate) const DEFAULT_BITCOIND_USERNAME: &str = "foo";

/// Default bitcoind password
pub(crate) const DEFAULT_BITCOIND_PASSWORD: &str = "bar";

/// Parameters to configure Bitcoind.
#[derive(Debug, Clone, Args, PartialEq, Eq, Serialize, Deserialize)]
#[clap(next_help_heading = "Bitcoind")]
pub struct BitcoindArgs {
    /// bitcoind RPC url
    ///
    /// The url of the bitcoind server.
    #[arg(default_value_t=Url::parse(DEFAULT_BITCOIND_URL).expect("valid url"), value_parser = parse_url, long = "bitcoind.url", name = "bitcoind.url", value_name = "BITCOIND_URL", env = "RETH_BITCOIND_URL"
    )]
    pub url: Url,

    /// Btcd username
    ///
    /// The username of the bitcoind server.
    #[arg(
        default_value_t = DEFAULT_BITCOIND_USERNAME.into(),
        long = "bitcoind.username",
        name = "bitcoind.username",
        value_name = "BITCOIND_USERNAME",
        env = "RETH_BITCOIND_USERNAME"
    )]
    pub username: String,

    /// Btcd password
    ///
    /// The password of the bitcoind server.
    #[arg(
        default_value_t = DEFAULT_BITCOIND_PASSWORD.into(),
        long = "bitcoind.password",
        name = "bitcoind.password",
        value_name = "BITCOIND_PASSWORD",
        env = "RETH_BITCOIND_PASSWORD"
    )]
    pub password: String,

    /// ZMQ address for bitcoind
    #[arg(
        default_value_t = Url::parse(DEFAULT_BITCOIND_ZMQ_HASH_BLOCK_ADDRESS).expect("valid url"),
        long = "bitcoind.zmq.hash-block-address",
        name = "bitcoind.zmq.hash-block-address",
        value_name = "BITCOIND_ZMQ_HASH_BLOCK_ADDRESS",
        env = "BITCOIND_ZMQ_HASH_BLOCK_ADDRESS"
    )]
    pub zmq_hash_block_address: Url,
}

impl Default for BitcoindArgs {
    fn default() -> Self {
        Self {
            url: Url::parse(DEFAULT_BITCOIND_URL).expect("valid url"),
            username: DEFAULT_BITCOIND_USERNAME.into(),
            password: DEFAULT_BITCOIND_PASSWORD.into(),
            zmq_hash_block_address: DEFAULT_BITCOIND_ZMQ_HASH_BLOCK_ADDRESS
                .parse()
                .expect("valid url"),
        }
    }
}

impl From<BitcoindArgs> for BitcoindConfig {
    fn from(args: BitcoindArgs) -> Self {
        Self::new(args.url.clone(), args.username.clone(), args.password)
    }
}
