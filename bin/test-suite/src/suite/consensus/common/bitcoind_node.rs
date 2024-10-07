use super::{
    botanix_client::BotanixEthClient, create_temp_working_directory, kill_process_at_port,
};
use crate::{context::GlobalContext, suite::consensus::common::spawn_child_process};
use std::{fs, path::PathBuf, sync::Arc};
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
    pub working_directory: PathBuf,
    pub botanix_eth_client: Option<BotanixEthClient>,
    pub test_signal_tx: Sender<TestSignal>,
    pub bitcoind_user: String,
    pub bitcoind_password: String,
}

impl BitcoindNodeConfig {
    pub async fn new(
        test_signal_tx: Sender<TestSignal>,
        bitcoind_user: String,
        bitcoind_password: String,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            working_directory: create_temp_working_directory(),
            botanix_eth_client: None,
            test_signal_tx,
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
            "-regtest=1",
            &bitcoind_user,
            &bitcoind_pwd,
            "-server=1",
            "-txindex=1",
            "-fallbackfee=0.00005",
        ];

        Ok(SpawnedBitcoindProcess {
            child_process: spawn_child_process(command, args, &self.working_directory)?,
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
    let bitcoind_node = BitcoindNodeConfig::new(
        test_signal_tx,
        global_context.bitcoind_user.clone(),
        global_context.bitcoind_pass.clone(),
    )
    .await?;
    let datadir = bitcoind_node.working_directory.join("data");
    fs::create_dir_all(datadir.clone())?;
    Ok((bitcoind_node, tx))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::suite::consensus::common::{create_temp_working_directory, spawn_child_process};

    #[tokio::test]
    async fn run_core() {
        spawn_child_process(
            "bitcoind",
            ["-conf", "/home/evgeni/Documents/.bitcoin/bitcoind.conf"].to_vec(),
            create_temp_working_directory(),
        )
        .unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}
