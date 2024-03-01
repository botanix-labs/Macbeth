use clap::Args;
use reth_btc_wallet::bitcoind::BitcoindConfig;
use url::Url;

/// Parameters to configure RPC state cache.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[clap(next_help_heading = "RPC State Cache")]

pub struct BitcoindArgs {
    /// Btcd url
    ///
    /// The url of the bitcoind server.
    #[arg(long = "bitcoind.url", name = "bitcoind.url", value_name = "BITCOIND_URL")]
    pub url: Url,

    /// Btcd username
    ///
    /// The username of the bitcoind server.
    #[arg(
        long = "bitcoind.username",
        name = "bitcoind.username",
        value_name = "BITCOIND_USERNAME"
    )]
    pub username: String,

    /// Btcd password
    ///
    /// The password of the bitcoind server.
    #[arg(
        long = "bitcoind.password",
        name = "bitcoind.password",
        value_name = "BITCOIND_PASSWORD"
    )]
    pub password: String,
}

impl From<BitcoindArgs> for BitcoindConfig {
    fn from(value: BitcoindArgs) -> Self {
        BitcoindConfig::new(value.url.clone(), value.username.clone(), value.password.clone())
    }
}
