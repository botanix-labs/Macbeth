use super::{create_temp_working_directory, kill_process_at_port, Scope};
use crate::{
    context::GlobalContext,
    suite::consensus::{
        common::{events::get_unique_wallet_name, spawn_child_process},
        ConsensusIntegrationTestSuite,
    },
    utils::{generate_blocks, MIN_BLOCKS_COINBASE_MATURE},
};
use bitcoincore_rpc::RpcApi;
use reth_btc_wallet::bitcoind::{BitcoindClientFactory, BitcoindConfig, BitcoindFactory};
use std::{fs, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    process::{Child, Command},
    sync::broadcast::{channel, Sender},
};
use url::Url;

#[derive(Clone, Debug)]
pub enum Notifications {}

#[derive(Clone, Debug)]
pub enum TestSignal {
    Disconnect(),
    Reconnect(),
}

#[derive(Debug)]
pub struct SpawnedBitcoindProcess {
    pub child_process: Child,
    pub port: u16,
    pub working_directory: PathBuf,
}

impl SpawnedBitcoindProcess {
    pub async fn destroy_all_async(&mut self) {
        // kill the process
        let _ = self.child_process.kill().await;
        kill_process_at_port(self.port);

        if let Err(e) = std::fs::remove_dir_all(&self.working_directory) {
            warn!("Couldn't remove bitcoind db dir at {}: {}", self.working_directory.display(), e);
        }
    }

    pub async fn destroy_all_sync(&self) {
        // kill the process
        let pid = self.child_process.id().expect("Expected a process id");
        let _ = std::process::Command::new("kill")
            .arg("-9") // Use SIGKILL for immediate termination
            .arg(format!("{pid}"))
            .output();
        kill_process_at_port(self.port);
    }

    pub async fn stop(&mut self, bitcoind_user: &str, bitcoind_password: &str) {
        let status = Command::new("bitcoin-cli")
            .arg("-regtest")
            .arg(format!("-rpcuser={}", bitcoind_user))
            .arg(format!("-rpcpassword={}", bitcoind_password))
            .arg("-rpcport=18443")
            .arg("stop")
            .status()
            .await
            .expect("to stop bitcoind");

        if !status.success() {
            error!("Failed to stop bitcoind: {:?}", status);
            panic!("Failed to stop bitcoind");
        }
    }
}

#[derive(Clone, Debug)]
pub struct BitcoindNodeConfig {
    pub working_directory: PathBuf,
    pub test_signal_tx: Sender<TestSignal>,
    pub bitcoind_url: Url,
    pub bitcoind_user: String,
    pub bitcoind_password: String,
    pub wallet_name: String,
}

impl BitcoindNodeConfig {
    pub async fn new(
        test_signal_tx: Sender<TestSignal>,
        bitcoind_url: Url,
        bitcoind_user: String,
        bitcoind_password: String,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            working_directory: create_temp_working_directory()?,
            test_signal_tx,
            bitcoind_url,
            bitcoind_user,
            bitcoind_password,
            wallet_name: get_unique_wallet_name(),
        })
    }

    pub fn spawn_service(&self) -> anyhow::Result<SpawnedBitcoindProcess> {
        // prepare run arguments
        let command = "bitcoind";
        let datadir =
            format!("-datadir={}", self.working_directory.join("data").display().to_string());
        let bitcoind_user = format!("-rpcuser={}", self.bitcoind_user);
        let bitcoind_pwd = format!("-rpcpassword={}", self.bitcoind_password);
        let args = vec![
            &datadir,
            "-chain=regtest",
            &bitcoind_user,
            &bitcoind_pwd,
            "-rpcport=18443",
            "-rpcbind=::1",
            "-server=1",
            "-txindex=1",
            "-fallbackfee=0.00005",
            "-persistmempool=0",
        ];

        Ok(SpawnedBitcoindProcess {
            child_process: spawn_child_process(
                Scope::Bitcoind,
                command,
                args,
                &self.working_directory,
            )?,
            port: 18443, // Note: using default port
            working_directory: self.working_directory.clone(),
        })
    }

    // this re-starts an existing stopped bitcoind instance and updates the local context
    pub async fn re_start(&mut self, suite: &mut ConsensusIntegrationTestSuite) {
        match self.spawn_service() {
            Ok(bitcoind_process) => {
                tokio::time::sleep(Duration::from_secs(6)).await;

                let bitcoind_factory = BitcoindClientFactory::new(BitcoindConfig::new(
                    suite.global_context.bitcoind_url.clone(),
                    suite.global_context.bitcoind_user.clone(),
                    suite.global_context.bitcoind_pass.clone(),
                ));
                let bitcoind_client =
                    bitcoind_factory.build_and_connect().expect("to build and connect client");
                bitcoind_client.load_wallet(&self.wallet_name).expect("wallet exists");

                // update local context
                suite.local_context.bitcoind_process = Some(bitcoind_process);
                suite.local_context.bitcoind_node = Some(self.clone());
                // Note: this field is not currently used in any tests
                suite.local_context.bitcoind_notification = None;
            }
            Err(e) => {
                error!("Failed to re-start bitcoind: {:?}", e);
                panic!();
            }
        };
    }
}

impl BitcoindNodeConfig {
    pub async fn setup_wallet(&self, bitcoin_client: &impl RpcApi) -> anyhow::Result<()> {
        match bitcoin_client.create_wallet(&self.wallet_name, None, None, None, None) {
            Ok(res) => {
                tracing::info!("Created wallet: {} with result {res:?}", &self.wallet_name);
            }
            Err(e) => {
                let err_msg = e.to_string();
                // Load the wallet if it already exists
                if err_msg.contains("already exists") || err_msg.contains("already loaded") {
                    tracing::info!("Wallet {} already loaded or existing", &self.wallet_name);
                } else {
                    tracing::info!("Loading wallet {} ...", &self.wallet_name);
                    bitcoin_client
                        .load_wallet(&self.wallet_name)
                        .map_err(|e| anyhow::anyhow!("Failed to create wallet: {}", e))?;
                }
            }
        }
        // Fund the wallet
        generate_blocks(bitcoin_client, MIN_BLOCKS_COINBASE_MATURE).await;
        Ok(())
    }
}

pub async fn create_bitcoind_node(
    global_context: Arc<GlobalContext>,
) -> anyhow::Result<(BitcoindNodeConfig, tokio::sync::broadcast::Sender<Notifications>)> {
    let (tx, _rx) = tokio::sync::broadcast::channel::<Notifications>(100);
    let (test_signal_tx, _test_signal_rx) = channel::<TestSignal>(10);
    let bitcoind_node = BitcoindNodeConfig::new(
        test_signal_tx,
        global_context.bitcoind_url.clone(),
        global_context.bitcoind_user.clone(),
        global_context.bitcoind_pass.clone(),
    )
    .await?;
    let datadir = bitcoind_node.working_directory.join("data");
    fs::create_dir_all(datadir.clone())?;
    Ok((bitcoind_node, tx))
}
