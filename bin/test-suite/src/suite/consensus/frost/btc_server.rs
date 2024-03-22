use std::{
    path::{Path, PathBuf},
    process::Stdio,
    vec,
};
use tokio::process::{Child, Command};

#[derive(Debug)]
pub struct SpawnedBtcServer {
    pub port: u16,
    pub db_path: PathBuf,
    pub child_process: Child,
}

fn spawn_btc_server(id: u16, address: String, db_path: PathBuf) -> Child {
    let db_path_arg = db_path.display().to_string();

    let mut working_directory = std::env::current_dir().unwrap();
    for _ in 0..2 {
        working_directory.pop();
    }
    working_directory.push("bin");
    working_directory.push("btc-server");

    let identifier = id.to_string();

    let command = "cargo";
    let args = vec![
        "run",
        "--",
        "--network",
        "testnet",
        "--db",
        db_path_arg.as_str(),
        "--identifier",
        identifier.as_str(),
        "--address",
        address.as_str(),
        "--min-signers",
        "2",
        "--max-signers",
        "3",
        "--toml",
        "./config.toml",
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
        std::fs::remove_dir_all(task.db_path.clone()).unwrap();
    }
}

pub fn spawn_n_btc_servers(n: u16) -> Vec<SpawnedBtcServer> {
    let mut tasks = vec![];
    for i in 0..n {
        let temp_db_path = tempfile::TempDir::new().expect("tempdir is okay").into_path();
        let db_path = Path::new(&temp_db_path).join(format!("db{}", i));
        let port = 8000 + i;
        let child_process = spawn_btc_server(i, format!("0.0.0.0:{}", port), db_path.clone());
        tasks.push(SpawnedBtcServer { db_path, port, child_process });
    }
    tasks
}
