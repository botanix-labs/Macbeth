use std::{
    path::{Path, PathBuf},
    process::Stdio,
    vec,
};
use tokio::process::{Child, Command};

use crate::config::Config;

#[derive(Debug)]
pub struct SpawnedBtcServer {
    pub port: u16,
    pub db_path: PathBuf,
    pub child_process: Child,
}

fn spawn_btc_server(id: u16, address: String, db_path: PathBuf, config: Config) -> Child {
    let db_path_arg = db_path.display().to_string();

    let mut working_directory = std::env::current_dir().unwrap();
    for _ in 0..2 {
        working_directory.pop();
    }
    working_directory.push("bin");
    working_directory.push("btc-server");

    let jwt_secret_file =
        config.jwt_secrets_dir.join(format!("{}.hex", id + 1)).display().to_string();

    let identifier = id.to_string();
    let frost_min_signers = config.frost_min_signers.to_string();
    let frost_max_signers = config.frost_max_signers.to_string();

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
        "--jwt-secret",
        jwt_secret_file.as_str(),
        "--bitcoind-url",
        "localhost:18443",
        "--bitcoind-user",
        config.bitcoind.username.as_str(),
        "--bitcoind-pass",
        config.bitcoind.password.as_str(),
        "--fee-rate-diff-percentage",
        "30",
        "--fall-back-fee-rate-sat-per-vbyte",
        "5",
    ];

    // Create a Command instance and set the working directory
    let mut cmd: Command = Command::new(command);
    cmd.args(&args).current_dir(working_directory).stdout(Stdio::piped());

    // Spawn the command and handle its output
    let child = cmd.spawn().unwrap();
    child
}

pub fn clean_db(tasks: &[SpawnedBtcServer]) {
    for task in tasks.iter() {
        if let Err(e) = std::fs::remove_dir_all(&task.db_path) {
            warn!("Couldn't remove db dir at {}: {}", task.db_path.display(), e);
        }
    }
}

pub fn spawn_n_btc_servers(n: u16, start_port: u16, config: Config) -> Vec<SpawnedBtcServer> {
    let mut tasks = vec![];
    for i in 0..n {
        let temp_db_path = tempfile::TempDir::new().expect("tempdir is okay").into_path();
        let db_path = Path::new(&temp_db_path).join(format!("db{}", i));
        let port = start_port + i;
        let child_process =
            spawn_btc_server(i, format!("0.0.0.0:{}", port), db_path.clone(), config.clone());
        tasks.push(SpawnedBtcServer { db_path, port, child_process });
    }
    tasks
}
