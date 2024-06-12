use crate::{context::GlobalContext, it_info_print};
use btc_server::config::{Config, GrpcConfig, TomlConfig};
use std::{
    fs::File,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    vec,
};
use tokio::process::{Child, Command};

pub const BTC_SERVER_START_PORT: u16 = 8000;

#[derive(Debug)]
pub struct SpawnedBtcServer {
    pub port: u16,
    pub db_path: PathBuf,
    pub child_process: Child,
}

async fn spawn_btc_server(
    global_context: Arc<GlobalContext>,
    id: u16,
    address: String,
    db_path: PathBuf,
) -> Child {
    let mut working_directory = std::env::current_dir().unwrap();
    for _ in 0..2 {
        working_directory.pop();
    }
    working_directory.push("bin");
    working_directory.push("btc-server");

    let app_config = Config {
        db: db_path.clone(),
        btc_network: bitcoin::Network::Regtest,
        identifier: id,
        address: address.clone(),
        max_signers: global_context.max_signers,
        min_signers: global_context.min_signers,
        jwt_secret: Some(global_context.jwt_dir.join(format!("{}.hex", id + 1))),
        bitcoind_url: global_context.bitcoind_url.clone(),
        bitcoind_user: global_context.bitcoind_user.clone(),
        bitcoind_pass: global_context.bitcoind_pass.clone(),
        fee_rate_diff_percentage: 30,
        fall_back_fee_rate_sat_per_vbyte: 3,
    };

    let config = TomlConfig { grpc: Default::default(), app: app_config };
    let config_file_path =
        db_path.parent().expect("parent temp dir").join(format!("config-{}.toml", id));

    config.write_to_path(config_file_path.clone()).await.expect("write config to path");

    let command = "cargo";
    let args = vec!["run", "--", "--config-path", config_file_path.to_str().expect("config path")];

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

pub async fn spawn_n_btc_servers(global_context: Arc<GlobalContext>) -> Vec<SpawnedBtcServer> {
    let mut tasks = vec![];
    for i in 0..global_context.instances {
        let temp_db_path = tempfile::TempDir::new().expect("tempdir is okay").into_path();
        let db_path = Path::new(&temp_db_path).join(format!("db{}", i));
        let port = BTC_SERVER_START_PORT + i;
        let child_process = spawn_btc_server(
            global_context.clone(),
            i,
            format!("0.0.0.0:{}", port),
            db_path.clone(),
        )
        .await;
        tasks.push(SpawnedBtcServer { db_path, port, child_process });
    }
    tasks
}
