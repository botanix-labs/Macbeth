use clap::Args;
use reth_btc_wallet::bitcoind::BitcoindConfig;
use serde::{Deserialize, Serialize};
use url::Url;

use super::utils::parse_url;

/// Default bitcoind url
pub(crate) const DEFAULT_BITCOIND_URL: &str = "localhost:18443";

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
    #[arg(default_value_t=Url::parse(DEFAULT_BITCOIND_URL).expect("valid url"), value_parser = parse_url, long = "bitcoind.url", name = "bitcoind.url", value_name = "BITCOIND_URL")]
    pub url: Url,

    /// Btcd username
    ///
    /// The username of the bitcoind server.
    #[arg(
        default_value_t = DEFAULT_BITCOIND_USERNAME.into(),
        long = "bitcoind.username",
        name = "bitcoind.username",
        value_name = "BITCOIND_USERNAME"
    )]
    pub username: String,

    /// Btcd password
    ///
    /// The password of the bitcoind server.
    #[arg(
        default_value_t = DEFAULT_BITCOIND_PASSWORD.into(),
        long = "bitcoind.password",
        name = "bitcoind.password",
        value_name = "BITCOIND_PASSWORD"
    )]
    pub password: String,
}

impl Default for BitcoindArgs {
    fn default() -> Self {
        BitcoindArgs {
            url: Url::parse(DEFAULT_BITCOIND_URL).expect("valid url"),
            username: DEFAULT_BITCOIND_USERNAME.into(),
            password: DEFAULT_BITCOIND_PASSWORD.into(),
        }
    }
}

impl From<BitcoindArgs> for BitcoindConfig {
    fn from(args: BitcoindArgs) -> Self {
        BitcoindConfig::new(args.url.clone(), args.username.clone(), args.password.clone())
    }
}
