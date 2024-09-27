use crate::context::GlobalContext;
use anyhow::Context;
use reth::consensus_common::utils::unix_timestamp;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    vec,
};
use tokio::process::Child;

use super::spawn_child_process;

pub const BTC_SERVER_START_PORT: u16 = 8000;

#[derive(Debug)]
pub struct SpawnedBtcServer {
    pub port: u16,
    pub db_path: PathBuf,
    pub child_process: Child,
}

fn spawn_btc_server(
    global_context: Arc<GlobalContext>,
    id: u16,
    port: u16,
    db_path: PathBuf,
) -> anyhow::Result<SpawnedBtcServer> {
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
        "3",
    ];

    Ok(SpawnedBtcServer {
        child_process: spawn_child_process(command, args, working_directory)?,
        db_path,
        port,
    })
}

pub fn clean_db(processes: &[SpawnedBtcServer]) {
    for processes in processes.iter() {
        if let Err(e) = std::fs::remove_dir_all(&processes.db_path) {
            warn!("Couldn't remove db dir at {}: {}", processes.db_path.display(), e);
        }
    }
}

pub fn spawn_n_btc_servers(
    global_context: Arc<GlobalContext>,
) -> anyhow::Result<Vec<SpawnedBtcServer>> {
    let mut processes = vec![];
    for i in 0..global_context.instances {
        let temp_db_path = tempfile::TempDir::new()
            .context("error creating tempdir")?
            .into_path()
            .join(format!("_{}", unix_timestamp().to_string()));
        std::fs::create_dir_all(&temp_db_path).context("failed to create tempdir db subdir")?;
        let db_path = Path::new(&temp_db_path).join(format!("db{}", i));

        let port = BTC_SERVER_START_PORT + i;
        let child_process = spawn_btc_server(global_context.clone(), i, port, db_path.clone())?;
        processes.push(child_process);
    }
    Ok(processes)
}
