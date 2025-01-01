use anyhow::{Context as AnyhowContext, Result};
use url::Url;

use crate::{
    config::{CliArgs, Config},
    suite::RunSuite,
};

pub const BOTANIX_FEE_RECEIPIENT: &str = "0xb8c03cb8C9bAC79c53926E3C66344C13452105f5";
pub const BTC_NETWORK: &str = "regtest";
pub const BITCOIND_URL: &str = "http://127.0.0.1:18443";
pub const BITCOIND_USER: &str = "foo";
pub const BITCOIND_PASS: &str = "bar";

pub struct GlobalContext {
    pub test_suite_id: uuid::Uuid,
    pub dry_run: bool,
    pub fed_instances: u16,
    pub rpc_instances: u16,
    pub run_suite: RunSuite,
    pub timeout: u64,
    pub min_signers: u16,
    pub max_signers: u16,
    pub max_snapshot_size_bytes: usize,
    pub snapshot_chunk_size_bytes: usize,
    pub snapshot_keep_recent: u64,
    pub btc_network: String,
    pub bitcoind_url: Url,
    pub bitcoind_user: String,
    pub bitcoind_pass: String,
    pub botanix_fee_recipient: String,
    pub features: String, // space delimited String
}

impl GlobalContext {
    pub async fn new(args: CliArgs) -> Result<Self> {
        let mut _config =
            Config::new(args.config.clone()).await.context("Failed to load config")?;
        // update config using envs
        _config.from_envs();

        // compute instances and min/max signers
        let frost_max_signers = args.max_signers;
        let fed_instances = frost_max_signers; // this is the total number of instances to be spawned (poa nodes and btc servers)
        let frost_min_signers = ((frost_max_signers - 1).min(args.min_signers)).max(2); //  value must be in the bounds: [2; value; max_signers - 1]
        assert!(frost_max_signers >= frost_min_signers, "frost signers rule violated");

        Ok(Self {
            test_suite_id: uuid::Uuid::new_v4(),
            dry_run: args.dry_run,
            fed_instances,
            rpc_instances: args.rpc_nodes,
            run_suite: args.run_suite,
            timeout: args.timeout,
            min_signers: frost_min_signers,
            max_signers: frost_max_signers,
            max_snapshot_size_bytes: args.max_snapshot_size_bytes,
            snapshot_chunk_size_bytes: args.snapshot_chunk_size_bytes,
            snapshot_keep_recent: args.snapshot_keep_recent,
            btc_network: BTC_NETWORK.to_string(),
            bitcoind_url: BITCOIND_URL.parse().context("Failed to parse BITCOIND_URL to an Url")?,
            bitcoind_user: BITCOIND_USER.to_string(),
            bitcoind_pass: BITCOIND_PASS.to_string(),
            botanix_fee_recipient: BOTANIX_FEE_RECEIPIENT.to_string(),
            features: args.features,
        })
    }

    pub fn bitcoind_rpc(&self) -> bitcoincore_rpc::Client {
        let host = self.bitcoind_url.host_str().unwrap_or_default().to_owned();
        let port = self.bitcoind_url.port_or_known_default().unwrap_or_default().to_owned();
        let url = format!("{}:{}", host, port);
        let auth =
            bitcoincore_rpc::Auth::UserPass(self.bitcoind_user.clone(), self.bitcoind_pass.clone());
        bitcoincore_rpc::Client::new(&url, auth).expect("bitcoind client")
    }
}
