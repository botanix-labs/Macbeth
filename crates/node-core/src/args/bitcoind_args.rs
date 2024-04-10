use clap::Args;
use reth_btc_wallet::bitcoind::BitcoindConfig;
use url::Url;

/// Parameters to configure Bitcoind.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[clap(next_help_heading = "Bitcoind")]

pub struct BitcoindArgs {
    /// bitcoind RPC url
    ///
    /// The url of the bitcoind server.
    #[arg(long = "bitcoind.url", name = "bitcoind.url", value_name = "BITCOIND_URL")]
    pub url: Url,

    /// bitcoind RPC cookie file path
    ///
    /// The path of the cookie of the bitcoind server.
    #[arg(
        long = "bitcoind.cookie",
        name = "bitcoind.cookie",
        value_name = "BITCOIND_COOKIE"
    )]
    pub cookie: String,
}

impl From<BitcoindArgs> for BitcoindConfig {
    fn from(args: BitcoindArgs) -> Self {
        BitcoindConfig::new(args.url, args.cookie)
    }
}
