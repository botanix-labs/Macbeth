use botanix_cli_parsers::parsers::parse_ethereum_address;
use clap::Parser;
use reth_primitives::Address;
use std::path::PathBuf;

use crate::state_sync::StateSyncArgs;

#[derive(Clone, Debug, Parser)]
pub struct PoaNodeArgs {
    /// The path to the configuration file to use for network properties.
    #[arg(
        long,
        value_name = "NETWORK_CONFIG_FILE",
        env = "RETH_NETWORK_CONFIG_PATH",
        verbatim_doc_comment
    )]
    pub network_config_path: Option<PathBuf>,

    /// Indicates whether we are running in testnet or not.
    #[arg(long, value_name = "IS_TESTNET", env = "RETH_TESTNET")]
    pub is_testnet: bool,

    /// Indicates whether we are running in devnet or not.
    #[arg(long, value_name = "IS_DEVNET", env = "RETH_DEVNET")]
    pub is_devnet: bool,

    /// The NTP server url
    #[arg(
        long,
        value_name = "NTP_SERVER",
        env = "RETH_NTP_SERVER",
        default_value = "time.cloudflare.com"
    )]
    pub ntp_server: String,

    /// The path to the configuration file for the federation setup.
    #[arg(
        long,
        value_name = "FEDERATION_CONFIG_FILE",
        env = "RETH_FEDERATION_CONFIG_FILE",
        verbatim_doc_comment
    )]
    pub federation_config_path: PathBuf,

    /// Run in federation mode. Only the nodes in the federation will be able to produce blocks.
    /// Only nodes defined in chain.toml can enable this flag
    #[arg(
        long,
        value_name = "FEDERATION_MODE",
        env = "RETH_FEDERATION_MODE",
        default_value = "false"
    )]
    pub federation_mode: bool,

    /// All state sync related arguments
    #[command(flatten)]
    pub state_sync: StateSyncArgs,

    /// The path to the configuration file to use for network properties.
    #[arg(
        long,
        value_name = "BITCOIND_CONFIG_FILE",
        env = "RETH_BITCOIND_CONFIG_PATH",
        verbatim_doc_comment
    )]
    pub bitcoind_config_path: Option<PathBuf>,

    /// ABCI client host to listen on
    #[arg(long, value_name = "ABCI_HOST", env = "RETH_ABCI_HOST", default_value_t = String::from("0.0.0.0"))]
    pub abci_host: String,

    /// ABCI client port to listen on
    #[arg(long, value_name = "ABCI_PORT", env = "RETH_ABCI_PORT", default_value_t = 26658)]
    pub abci_port: u16,

    /// `CometBFT` RPC Port
    #[arg(
        long,
        value_name = "COMETBFT_RPC_PORT",
        env = "RETH_COMETBFT_RPC_PORT",
        default_value_t = 26657
    )]
    pub cometbft_rpc_port: u16,

    // TODO parse to a better type
    /// `CometBFT` RPC Host
    #[arg(long, value_name = "COMETBFT_RPC_HOST", env = "RETH_COMETBFT_RPC_HOST", default_value_t = String::from("127.0.0.1"))]
    pub cometbft_rpc_host: String,

    /// Block fee recipient address.
    ///
    /// The input should be a hex string with exactly 40 hex characters.
    /// An optional "0x" prefix is allowed.
    #[arg(
        long,
        value_name = "BLOCK_FEE_RECIPIENT_ADDRESS",
        env = "RETH_BLOCK_FEE_RECIPIENT_ADDRESS",
        value_parser = parse_ethereum_address,
    )]
    pub block_fee_recipient_address: Option<Address>,
}
