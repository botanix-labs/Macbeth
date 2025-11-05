use super::{kill_process_at_port, Scope};
use crate::{it_info_print, suite::consensus::common::spawn_child_process};
use anyhow::Context;
use reth::consensus_common::utils::unix_timestamp;
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::process::Child;
pub const PEGIN_RECOVERY_DEFAULT_PORT: u16 = 50052;

#[derive(Debug)]
pub struct SpawnedPeginRecoveryProcess {
    pub child_process: Child,
    pub port: u16,
    pub working_directory: PathBuf,
}

impl SpawnedPeginRecoveryProcess {
    pub async fn destroy_all_async(&mut self) {
        // kill the process
        let _ = self.child_process.kill().await;
        kill_process_at_port(self.port);

        if let Err(e) = std::fs::remove_dir_all(&self.working_directory) {
            warn!(
                "Couldn't remove pegin recovery db dir at {}: {}",
                self.working_directory.display(),
                e
            );
        }
    }
}

#[derive(Debug, Clone)]
pub struct PeginRecoveryNodeConfig {
    pub working_directory: PathBuf,
    pub port: u16,
    pub bitcoin_rpc_url: String,
    pub bitcoin_rpc_user: String,
    pub bitcoin_rpc_password: String,
    pub fallback_fee_rate: u64,
}

impl PeginRecoveryNodeConfig {
    pub fn spawn_service(&self) -> anyhow::Result<SpawnedPeginRecoveryProcess> {
        // Get project root directory
        let mut project_root = std::env::current_dir().unwrap();
        for _ in 0..2 {
            project_root.pop();
        }

        // prepare run arguments
        let command = "target/debug/pegin-recovery";
        let binary_abs_path = project_root.join(std::path::Path::new(command));
        if !std::fs::exists(&binary_abs_path)? {
            return Err(anyhow::anyhow!("pegin-recovery binary not found at {}. Please compile it first before running the test-suite", binary_abs_path.display().to_string()));
        }

        let db_path = self.working_directory.join("db");
        std::fs::create_dir_all(&db_path)
            .context("Failed to create pegin recovery db directory")?;
        let db = db_path.to_string_lossy();
        let port = self.port.to_string();
        let fallback_fee_rate = self.fallback_fee_rate.to_string();

        let args = vec![
            "--db",
            &db,
            "--port",
            &port,
            "--bitcoin-rpc-url",
            &self.bitcoin_rpc_url,
            "--bitcoin-rpc-user",
            &self.bitcoin_rpc_user,
            "--bitcoin-rpc-password",
            &self.bitcoin_rpc_password,
            "--fallback-fee-rate",
            &fallback_fee_rate,
        ];

        it_info_print!(
            " Starting Pegin Recovery Service",
            format!("Port: {}, DB: {}", self.port, db_path.display())
        );

        Ok(SpawnedPeginRecoveryProcess {
            child_process: spawn_child_process(
                Scope::PeginRecovery(0),
                command,
                args,
                project_root,
            )?,
            port: self.port,
            working_directory: self.working_directory.clone(),
        })
    }
}

pub fn create_pegin_recovery_node(
    global_context: std::sync::Arc<crate::context::GlobalContext>,
) -> anyhow::Result<PeginRecoveryNodeConfig> {
    let temp_db_path = TempDir::new()
        .context("error creating tempdir")?
        .keep()
        .join(format!("pegin_recovery_{}", unix_timestamp().to_string()));
    std::fs::create_dir_all(&temp_db_path).context("failed to create tempdir with db subdir")?;

    Ok(PeginRecoveryNodeConfig {
        working_directory: temp_db_path,
        port: PEGIN_RECOVERY_DEFAULT_PORT,
        bitcoin_rpc_url: global_context.bitcoind_url.to_string(),
        bitcoin_rpc_user: global_context.bitcoind_user.clone(),
        bitcoin_rpc_password: global_context.bitcoind_pass.clone(),
        fallback_fee_rate: 5, // Default to 5 sat/vB for tests
    })
}
