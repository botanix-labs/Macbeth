use super::error::Error;
use crate::{
    it_info_print,
    suite::consensus::ConsensusIntegrationTestSuite,
    utils::{generate_blocks, get_gateway_address_with_retry},
};
use bitcoin::{consensus::encode::deserialize_hex, Amount, OutPoint, Transaction, TxOut};
use bitcoincore_rpc::RpcApi;
use botanix_chainspec::constants::BOTANIX_TESTNET;
use btcserverlib::{database, database::version::UtxoVersion};
use ethers::{prelude::Provider, providers::Http};
use frost_secp256k1_tr as frost;
use pegin_recovery_client::{self, PeginRecoveryServiceClient};
use std::{fs, path::Path, process::Command, str::FromStr, time::Duration};
use tonic::transport::Channel;
pub async fn test_pegin_recovery(suite: &mut ConsensusIntegrationTestSuite) -> anyhow::Result<()> {
    it_info_print!("Starting pegin recovery test...");

    // Simulate test steps
    // inital fed memebers setup , bitcoind, btc server setup and do DKG
    // Generate gateway address for random ETH address
    // Fund the gateway address with some BTC
    // Simulate pegin recovery process this process includes

    // - Detecting the pegin transaction on the Bitcoin network
    // - Verifying the transaction details
    // - creating a PSBT transaction to move BTC into PRS detination address
    // - Signing the PSBT transaction using federation members
    // - Broadcasting the signed transaction to the Bitcoin network
    // - Confirming the transaction and updating the state accordingly
    // - Validating that the pegin recovery was successful
    //
    //

    // DKG Setup (copied from frost_e2e but stopping after key generation)
    let pegin_conf_depth = BOTANIX_TESTNET.bitcoin_checkpoint_confirmation_depth;
    it_info_print!("Pegin Confirmation Depth", pegin_conf_depth);

    // Set up regtest connection
    let bitcoind_rpc = suite.global_context.bitcoind_rpc();
    tokio::time::sleep(Duration::from_secs(5)).await;

    let test_fed_members = suite
        .local_context
        .poa_nodes
        .as_ref()
        .expect("test federation member configurations")
        .clone();

    // Set up dummy eth address for testing
    let eth_destination = ethers::core::types::Address::random();

    // Provider to one of the federation members
    let provider = Provider::<Http>::try_from(format!(
        "http://localhost:{}",
        test_fed_members.get(&0).unwrap().rpc_port
    ))
    .expect("could not instantiate HTTP Provider");

    // Get gateway address (this confirms DKG is complete)
    let gateway_address_response =
        get_gateway_address_with_retry(provider.clone(), eth_destination.0.into(), 100).await?;
    it_info_print!("Gateway Address Response", gateway_address_response);
    it_info_print!("Aggregate Public Key", gateway_address_response.aggregate_public_key);

    // print balance
    let balance = bitcoind_rpc.get_balance(None, None).expect("get balance");
    it_info_print!("Bitcoin balance", balance);

    // Send some bitcoin to that gateway address
    let btc_address = bitcoin::Address::from_str(gateway_address_response.gateway_address.as_str())
        .expect("valid btc_address")
        .assume_checked();
    let pegin_txid = bitcoind_rpc
        .send_to_address(&btc_address, Amount::ONE_BTC, None, None, Some(true), None, Some(1), None)
        .expect("valid send");
    // Generate some block to confirm it
    generate_blocks(&bitcoind_rpc, 1 + pegin_conf_depth).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // retrieve the transaction
    let tx_res = bitcoind_rpc.get_transaction(&pegin_txid, None).expect("valid tx");
    assert!(tx_res.info.confirmations > 1);
    let pegin_tx = tx_res.transaction().expect("valid tx");
    it_info_print!("Bitcoin pegin Tx", pegin_tx);
    it_info_print!("Gateway Data", gateway_address_response);
    it_info_print!("Gateway Data Pub key", gateway_address_response.aggregate_public_key);

    let (vout, pegin_output) = pegin_tx
        .output
        .iter()
        .enumerate()
        .find(|(_, o)| o.script_pubkey == btc_address.script_pubkey())
        .unwrap();
    let amount = pegin_output.value;
    it_info_print!("Btc Amount", amount);

    //export all fedration databases to tempoary directories
    it_info_print!("Starting Phase 2: Database Export");

    // Export all federation databases to temporary directories
    let temp_db_dir =
        std::env::temp_dir().join(format!("pegin_recovery_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_db_dir).map_err(|e| {
        super::error::Error::TestVectorExport(format!("Failed to create temp directory: {}", e))
    })?;

    let btc_processes = suite.local_context.btc_processes.as_ref().ok_or_else(|| {
        super::error::Error::TestVectorExport("No BTC server processes found".to_string())
    })?;

    let mut fed_db_paths = Vec::new();

    for (index, process) in btc_processes.iter().enumerate() {
        let target_db_path = temp_db_dir.join(format!("fed_member_{}", index));
        copy_dir_all(&process.db_path, &target_db_path).map_err(|e| {
            super::error::Error::TestVectorExport(format!("Failed to copy database: {}", e))
        })?;
        it_info_print!(
            "Exported federation database",
            format!("fed_member_{} -> {}", index, target_db_path.display())
        );
        fed_db_paths.push(target_db_path);
    }

    it_info_print!(
        "Federation databases exported",
        format!("Count: {}, Location: {}", fed_db_paths.len(), temp_db_dir.display())
    );

    // Export key packages for all federation members
    it_info_print!("Starting Phase 3: Key Package Export");
    let fed_key_package_paths = export_key_packages_for_all_members(&fed_db_paths, &temp_db_dir)?;

    // Start pegin recovery service
    it_info_print!("Starting Phase 4: Pegin Recovery Service");
    const DEFAULT_PORT: u16 = 50052;

    let pegin_recovery_db_path = temp_db_dir.join("pegin_recovery_db");
    std::fs::create_dir_all(&pegin_recovery_db_path).map_err(|e| {
        Error::TestVectorExport(format!("Failed to create pegin recovery db directory: {}", e))
    })?;

    let mut pegin_recovery_process = Command::new("cargo")
        .args(&[
            "run",
            "--package",
            "pegin-recovery",
            "--",
            "--db",
            &pegin_recovery_db_path.to_string_lossy(),
            "--port",
            &DEFAULT_PORT.to_string(),
            "--log-level",
            "info",
        ])
        .spawn()
        .map_err(|e| {
            Error::TestVectorExport(format!("Failed to start pegin recovery service: {}", e))
        })?;

    // Wait for service to start up
    tokio::time::sleep(Duration::from_secs(3)).await;

    //create pegin recovery service client
    let client = pegin_recovery_client::PeginRecoveryServiceClient::connect(format!(
        "http://localhost:{}",
        DEFAULT_PORT
    ))
    .await
    .map_err(Error::ServerConnect)?;

    //import keyshare packcage for each exported federation member key package
    for (index, db_path) in fed_key_package_paths.iter().enumerate() {
        let frost_identifier = frost::Identifier::derive((index as u16).to_le_bytes().as_slice())
            .expect("valid frost identifier")
            .serialize();
        let key_package_data = std::fs::read(db_path).map_err(|e| {
            Error::TestVectorExport(format!("Failed to read key package file: {}", e))
        })?;

        // Deserialize the ExportedKeyPackage from CBOR format
        let exported_package: btcserverlib::database::ExportedKeyPackage =
            ciborium::from_reader(key_package_data.as_slice()).map_err(|e| {
                Error::TestVectorExport(format!("Failed to deserialize key package: {}", e))
            })?;

        client
            .clone()
            .import_key_share(tonic::Request::new(pegin_recovery_client::ImportKeyShareRequest {
                multisig_id: gateway_address_response.aggregate_public_key.as_bytes().to_vec(),
                node_identifier: frost_identifier.to_vec(),
                passphrase: "test_passphrase".to_string(),
                export: Some(pegin_recovery_client::ExportedKeyPackage {
                    version: exported_package.version as u32,
                    iv: exported_package.iv.to_vec(),
                    enc_key_package: exported_package.enc_key_package,
                    enc_pk_package: exported_package.enc_pk_package,
                }),
            }))
            .await
            .map_err(Error::Request)?;
    }

    it_info_print!("Pegin recovery test completed successfully.");

    // Clean up: kill the pegin recovery service
    let _ = pegin_recovery_process.kill();

    Ok(())
}

/// Export key packages for all federation database paths
fn export_key_packages_for_all_members(
    fed_db_paths: &[std::path::PathBuf],
    temp_db_dir: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, super::error::Error> {
    let mut output_paths = Vec::new();

    for (index, db_path) in fed_db_paths.iter().enumerate() {
        let output_path = temp_db_dir.join(format!("key_package_fed_member_{}.bin", index));

        it_info_print!(
            "Exporting key package",
            format!("Fed member {} -> {}", index, output_path.display())
        );

        // Run the btc-utils export-key-package command
        let output = Command::new("cargo")
            .args(&[
                "run",
                "--package",
                "btc-server",
                "--bin",
                "btc-utils",
                "--",
                "export-key-package",
                "--db",
                &db_path.to_string_lossy(),
                "--output",
                &output_path.to_string_lossy(),
                "--passphrase",
                "test_passphrase",
            ])
            .output()
            .map_err(|e| {
                super::error::Error::TestVectorExport(format!(
                    "Failed to execute CLI command: {}",
                    e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(super::error::Error::TestVectorExport(format!(
                "CLI command failed with status {}: {}",
                output.status, stderr
            )));
        }

        it_info_print!(
            "Successfully exported key package",
            format!("Fed member {} to {}", index, output_path.display())
        );

        output_paths.push(output_path);
    }

    Ok(output_paths)
}

/// Recursively copy a directory
fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}
