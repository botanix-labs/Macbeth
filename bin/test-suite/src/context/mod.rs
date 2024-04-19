use std::path::PathBuf;

use anyhow::{Context as AnyhowContext, Result};
use url::Url;

use crate::{
    config::{CliArgs, Config},
    suite::RunSuite,
};

pub struct GlobalContext {
    pub test_suite_id: uuid::Uuid,
    pub dry_run: bool,
    pub instances: u16,
    pub jwt_dir: PathBuf,
    pub run_suite: RunSuite,
    pub timeout: u64,
    pub min_signers: u16,
    pub max_signers: u16,
    pub btc_network: String,
    pub bitcoind_url: Url,
    pub bitcoind_user: String,
    pub bitcoind_pass: String,
}

impl GlobalContext {
    pub async fn new(args: CliArgs) -> Result<Self> {
        let mut _config =
            Config::new(args.config.clone()).await.context("Failed to load config")?;
        // update config using envs
        _config.from_envs();

        // compute instances and min/max signers
        let frost_max_signers = args.max_signers;
        let instances = frost_max_signers; // this is the total number of instances to be spawned (poa nodes and btc servers)
        let frost_min_signers = ((frost_max_signers - 1).min(args.min_signers)).max(2); //  value must be in the bounds: [2; value; max_signers - 1]
        assert!(frost_max_signers >= frost_min_signers, "frost signers rule violated");

        Ok(Self {
            test_suite_id: uuid::Uuid::new_v4(),
            dry_run: args.dry_run,
            instances,
            jwt_dir: args.jwt_dir.clone(),
            run_suite: args.run_suite,
            timeout: args.timeout,
            min_signers: frost_min_signers,
            max_signers: frost_max_signers,
            btc_network: args.btc_network,
            bitcoind_url: args.bitcoind_url,
            bitcoind_user: args.bitcoind_user,
            bitcoind_pass: args.bitcoind_pass,
        })
    }
}
