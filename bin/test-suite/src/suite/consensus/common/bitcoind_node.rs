use super::{create_temp_working_directory, kill_process_at_port, Scope};
use crate::{
    context::GlobalContext,
    suite::consensus::common::{events::BITCOIND_WALLET_NAME, spawn_child_process}, utils::{generate_blocks, MIN_BLOCKS_COINBASE_MATURE},
};
use bitcoincore_rpc::RpcApi;
use std::{fs, path::PathBuf, sync::Arc};
use tokio::{
    process::Child,
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
}

#[derive(Clone, Debug)]
pub struct BitcoindNodeConfig {
    pub working_directory: PathBuf,
    pub test_signal_tx: Sender<TestSignal>,
    pub bitcoind_url: Url,
    pub bitcoind_user: String,
    pub bitcoind_password: String,
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
}

impl BitcoindNodeConfig {
    pub async fn setup_wallet(&self, bitcoin_client: &impl RpcApi) -> anyhow::Result<()> {
        match bitcoin_client.create_wallet(BITCOIND_WALLET_NAME, None, None, None, None) {
            // Load the wallet if it already exists
            Err(e) => {
                if e.to_string().contains("wallet already exists") {
                    bitcoin_client.load_wallet(BITCOIND_WALLET_NAME)?;
                } else {
                    return Err(anyhow::anyhow!("Failed to create wallet: {}", e));
                }
            }
            Ok(_) => {}
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
