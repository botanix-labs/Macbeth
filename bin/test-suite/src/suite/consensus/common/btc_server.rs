use crate::context::GlobalContext;
use anyhow::Context;
use reth::consensus_common::utils::unix_timestamp;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    vec,
};
use tokio::process::Child;

use super::{kill_process_at_port, spawn_child_process, Scope};

pub const BTC_SERVER_START_PORT: u16 = 8000;
#[derive(Debug)]
pub struct SpawnedBtcServerProcess {
    pub port: u16,
    pub db_path: PathBuf,
    pub child_process: Child,
}

impl SpawnedBtcServerProcess {
    pub async fn destroy_all_async(&mut self) {
        // kill the process
        let _ = self.child_process.kill().await;
        // additionally make sure all ports used are freed
        kill_process_at_port(self.port);
        // delete the created db
        if let Err(e) = std::fs::remove_dir_all(&self.db_path) {
            warn!("Couldn't remove btc server db dir at {}: {}", self.db_path.display(), e);
        }
    }

    pub async fn destroy_all_sync(&mut self) {
        // kill the process
        let pid = self.child_process.id().expect("Expected a process id");
        let _ = std::process::Command::new("kill")
            .arg("-9") // Use SIGKILL for immediate termination
            .arg(format!("{pid}"))
            .output();
        // additionally make sure all ports used are freed
        kill_process_at_port(self.port);
        // delete the created db
        if let Err(e) = std::fs::remove_dir_all(&self.db_path) {
            warn!("Couldn't remove btc server db dir at {}: {}", self.db_path.display(), e);
        }
    }
}

fn spawn_btc_server_process(
    global_context: Arc<GlobalContext>,
    id: u16,
    port: u16,
    db_path: PathBuf,
) -> anyhow::Result<SpawnedBtcServerProcess> {
    let db_path_arg = db_path.display().to_string();

    let mut working_directory = std::env::current_dir().unwrap();
    for _ in 0..2 {
        working_directory.pop();
    }
    working_directory.push("bin");
    working_directory.push("btc-server");

    let identifier = id.to_string();
    let frost_max_signers = global_context.max_signers.to_string();
    let frost_min_signers = global_context.min_signers.to_string();
    let address = format!("0.0.0.0:{}", port);

    let command = "cargo";
    let args = vec![
        "run",
        "--",
        "--btc-network",
        "regtest",
        "--db",
        db_path_arg.as_str(),
        "--identifier",
        identifier.as_str(),
        "--address",
        address.as_str(),
        "--min-signers",
        frost_min_signers.as_str(),
        "--max-signers",
        frost_max_signers.as_str(),
        "--toml",
        "./config.toml",
        "--bitcoind-url",
        global_context.bitcoind_url.as_str(),
        "--bitcoind-user",
        global_context.bitcoind_user.as_str(),
        "--bitcoind-pass",
        global_context.bitcoind_pass.as_str(),
        "--fee-rate-diff-percentage",
        "30",
        "--fall-back-fee-rate-sat-per-vbyte",
        "5",
    ];

    Ok(SpawnedBtcServerProcess {
        child_process: spawn_child_process(Scope::BtcServer(id), command, args, working_directory)?,
        db_path,
        port,
    })
}

pub fn spawn_n_btc_server_processes(
    global_context: Arc<GlobalContext>,
) -> anyhow::Result<Vec<SpawnedBtcServerProcess>> {
    let mut processes = vec![];
    for i in 0..global_context.fed_instances {
        let temp_db_path = tempfile::TempDir::new()
            .context("error creating tempdir")?
            .into_path()
            .join(format!("_{}", unix_timestamp().to_string()));
        std::fs::create_dir_all(&temp_db_path).context("failed to create tempdir db subdir")?;
        let db_path = Path::new(&temp_db_path).join(format!("db{}", i));

        let port = BTC_SERVER_START_PORT + i;
        let child_process =
            spawn_btc_server_process(global_context.clone(), i, port, db_path.clone())?;
        processes.push(child_process);
    }
    Ok(processes)
}
