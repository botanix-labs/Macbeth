use super::{botanix_client::BotanixEthClient, kill_process_at_port, TemplateWriter};
use crate::{context::GlobalContext, suite::consensus::common::spawn_child_process};
use anyhow::Context;
use askama::Template;
use reth::consensus_common::utils::unix_timestamp;
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    process::Child,
    sync::broadcast::{channel, Sender},
};

#[derive(Clone, Debug)]
pub enum Notifications {}

#[derive(Clone, Debug)]
pub enum TestSignal {
    Disconnect(),
    Reconnect(),
}

// =============================== TEMPLATES =========================== //

#[derive(Template, Clone, Debug, Serialize)]
#[template(path = "bitcoind.conf", ext = "json", escape = "none")]
struct BitcoindConfigTemplate<'a> {
    datadir: &'a str,
    rpc_user: &'a str,
    rpc_password: &'a str,
}

impl TemplateWriter for BitcoindConfigTemplate<'_> {}

// ================================================================= //

#[derive(Debug)]
pub struct SpawnedBitcoindProcess {
    pub child_process: Child,
    pub port: u16,
}

impl SpawnedBitcoindProcess {
    pub async fn destroy_all_async(&mut self) {
        // kill the process
        let _ = self.child_process.kill().await;
        kill_process_at_port(self.port);
    }

    pub async fn destroy_all_sync(&mut self) {
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
    pub temp_path: PathBuf,
    pub botanix_eth_client: Option<BotanixEthClient>,
    pub test_signal_tx: Sender<TestSignal>,
}

impl BitcoindNodeConfig {
    pub async fn new(test_signal_tx: Sender<TestSignal>) -> anyhow::Result<Self> {
        Ok(Self {
            temp_path: {
                let ret = tempfile::TempDir::new()
                    .expect("tempdir is okay")
                    .into_path()
                    .join(format!("_{}", unix_timestamp().to_string()));
                std::fs::create_dir_all(&ret).expect("failed to create tempdir subdir");
                let bitcoind_conf_file = Path::new(&ret).join("bitcoin");
                fs::create_dir_all(&bitcoind_conf_file)?;
                bitcoind_conf_file
            },
            botanix_eth_client: None,
            test_signal_tx,
        })
    }

    pub fn spawn_service(&self) -> anyhow::Result<SpawnedBitcoindProcess> {
        // point to the relevant working directory
        let mut working_directory =
            std::env::current_dir().context("Error obtaining current directory")?;
        for _ in 0..2 {
            working_directory.pop();
        }

        // prepare run arguments
        let home_path = self.temp_path.to_path_buf();
        let home_path_str = home_path.display().to_string();
        let command = "bitcoind";
        let args = vec!["-conf", &home_path_str];

        Ok(SpawnedBitcoindProcess {
            child_process: spawn_child_process(command, args, working_directory)?,
            port: 18443, // Note: using default port
        })
    }
}

impl BitcoindNodeConfig {
    pub fn await_initialization(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

pub async fn create_bitcoind_node(
    global_context: Arc<GlobalContext>,
) -> anyhow::Result<(BitcoindNodeConfig, tokio::sync::broadcast::Sender<Notifications>)> {
    let (tx, _rx) = tokio::sync::broadcast::channel::<Notifications>(100);
    let (test_signal_tx, _test_signal_rx) = channel::<TestSignal>(10);
    let bitcoind_node = BitcoindNodeConfig::new(test_signal_tx).await?;

    // ~~~~~~~~~~~~~~~~~~ write bitcoind.conf file ~~~~~~~~~~~~~~~~~~
    BitcoindConfigTemplate {
        datadir: bitcoind_node.temp_path.display().to_string().as_str(),
        rpc_password: &global_context.bitcoind_pass.as_str(),
        rpc_user: global_context.bitcoind_user.as_str(),
    }
    .write_to_file(&bitcoind_node.temp_path, "bitcoind.conf")
    .context("Error writing bitcoind.conf to path")?;

    Ok((bitcoind_node, tx))
}
