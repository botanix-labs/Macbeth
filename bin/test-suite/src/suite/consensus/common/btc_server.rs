use crate::{context::GlobalContext, suite::consensus::common::is_port_free};
use anyhow::Context;
use btcserverlib::federation_args::{FedMemberPubKey, FederationTomlConfig};
use reth::consensus_common::utils::unix_timestamp;
use reth_primitives::Address;
use reth_rpc_types::PeerId;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    vec,
};
use tokio::process::Child;

use super::{kill_process_at_port, spawn_child_process, Scope};

pub const BTC_SERVER_START_PORT: u16 = 8000;
pub const BTC_SERVER_HTTP_PORT: u16 = 7000;

#[derive(Debug)]
pub struct SpawnedBtcServerProcess {
    pub btc_server_port: u16,
    pub db_path: PathBuf,
    pub child_process: Child,
}

impl SpawnedBtcServerProcess {
    pub async fn destroy_all_async(&mut self) {
        // kill the process
        let _ = self.child_process.kill().await;
        // additionally make sure all ports used are freed
        kill_process_at_port(self.btc_server_port);
        // delete the created db
        if let Err(e) = std::fs::remove_dir_all(&self.db_path) {
            warn!("Couldn't remove btc server db dir at {}: {}", self.db_path.display(), e);
        }
    }

    pub async fn destroy_all_sync(&self) {
        // kill the process
        let pid = self.child_process.id().expect("Expected a process id");
        let _ = std::process::Command::new("kill")
            .arg("-9") // Use SIGKILL for immediate termination
            .arg(format!("{pid}"))
            .output();
        // additionally make sure all ports used are freed
        kill_process_at_port(self.btc_server_port);
        // delete the created db
        if let Err(e) = std::fs::remove_dir_all(&self.db_path) {
            warn!("Couldn't remove btc server db dir at {}: {}", self.db_path.display(), e);
        }
    }
}

fn spawn_btc_server_process(
    global_context: Arc<GlobalContext>,
    members_keypairs: &Vec<(secp256k1::SecretKey, secp256k1::PublicKey, PeerId, Address)>,
    id: u16,
    btc_server_port: u16,
    db_path: PathBuf,
) -> anyhow::Result<SpawnedBtcServerProcess> {
    let db_path_arg = db_path.display().to_string();

    let mut working_directory = std::env::current_dir().unwrap();
    for _ in 0..2 {
        working_directory.pop();
    }

    let identifier = id.to_string();
    let coordinator = 0u16.to_string();

    let frost_max_signers = global_context.max_signers.to_string();
    let frost_min_signers = global_context.min_signers.to_string();
    let address = format!("0.0.0.0:{}", btc_server_port);
    let _http_port = (BTC_SERVER_HTTP_PORT + id).to_string();

    let command = "target/debug/btc-server";
    let binary_abs_path = working_directory.join(Path::new(command));
    if !std::fs::exists(&binary_abs_path)? {
        return Err(anyhow::anyhow!("btc-server binary not found at {}. Please compile it first before running the test-suite", binary_abs_path.display().to_string()));
    }

    // Create federation members
    let mut fed_members = vec![];
    for i in 0..global_context.fed_instances {
        let public_key =
            members_keypairs.get(i as usize).cloned().expect("To have keypair information").1;

        fed_members.push(FedMemberPubKey {
            key: public_key.to_string(),
            // Not needed
            socket_addr: String::new(),
        });
    }

    // Write federation config to tempfile
    let federation_config = FederationTomlConfig::new(
        fed_members,
        String::new(), // Not needed
        String::new(), // Not needed
        String::new(), // Not needed
    );

    let mut temp_federation = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(
        &mut temp_federation,
        toml::to_string(&federation_config)?.as_bytes(),
    )?;

    // Write the secret key to a tempfile
    let my_secret_key =
        members_keypairs.get(id as usize).cloned().expect("To have keypair information").0;

    let mut temp_secret_key = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(
        &mut temp_secret_key,
        my_secret_key.display_secret().to_string().as_bytes(),
    )?;

    let federation_path = temp_federation.path().to_str().unwrap().to_owned();
    let secret_key_path = temp_secret_key.path().to_str().unwrap().to_owned();

    let args = vec![
        "--btc-network",
        "regtest",
        "--db",
        db_path_arg.as_str(),
        "--identifier",
        identifier.as_str(),
        "--coordinator",
        coordinator.as_str(),
        "--federation-config-path",
        federation_path.as_str(),
        "--p2p-secret-key",
        secret_key_path.as_str(),
        "--address",
        address.as_str(),
        "--min-signers",
        frost_min_signers.as_str(),
        "--max-signers",
        frost_max_signers.as_str(),
        "--toml",
        "./bin/btc-server/config.toml",
        "--bitcoind-url",
        global_context.bitcoind_url.as_str(),
        "--bitcoind-user",
        global_context.bitcoind_user.as_str(),
        "--bitcoind-pass",
        global_context.bitcoind_pass.as_str(),
        "--fee-rate-diff-percentage",
        "50",
        "--fall-back-fee-rate-sat-per-vbyte",
        "5",
    ];

    // Keep the temp files alive for the duration of the test
    std::mem::forget(temp_federation);
    std::mem::forget(temp_secret_key);

    Ok(SpawnedBtcServerProcess {
        child_process: spawn_child_process(Scope::BtcServer(id), command, args, working_directory)?,
        db_path,
        btc_server_port,
    })
}

pub fn spawn_n_btc_server_processes(
    global_context: Arc<GlobalContext>,
    members_keypairs: &Vec<(secp256k1::SecretKey, secp256k1::PublicKey, PeerId, Address)>,
) -> anyhow::Result<Vec<SpawnedBtcServerProcess>> {
    let mut processes = vec![];
    for i in 0..global_context.fed_instances {
        let temp_db_path = tempfile::TempDir::new()
            .context("error creating tempdir")?
            .keep()
            .join(format!("_{}", unix_timestamp().to_string()));
        std::fs::create_dir_all(&temp_db_path)
            .context("failed to create tempdir with db subdir")?;
        let db_path = Path::new(&temp_db_path).join(format!("db{}", i));
        std::fs::create_dir_all(&db_path).context("failed to create tempdir with db subdir")?;
        let btc_server_port = BTC_SERVER_START_PORT + i;

        if !is_port_free(btc_server_port) {
            return Err(anyhow::anyhow!(
                "❌ BTC Server {} needs port {} but it's already in use by another process",
                i,
                btc_server_port
            ));
        }

        let child_process = spawn_btc_server_process(
            global_context.clone(),
            members_keypairs,
            i,
            btc_server_port,
            db_path.clone(),
        )?;
        processes.push(child_process);
    }
    Ok(processes)
}
