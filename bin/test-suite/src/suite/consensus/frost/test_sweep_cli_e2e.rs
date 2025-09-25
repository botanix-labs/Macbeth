use std::{fs, path::Path, process::Command, str::FromStr, time::Duration};

use bitcoin::{consensus::encode::deserialize_hex, Amount, OutPoint, Transaction, TxOut};
use bitcoincore_rpc::RpcApi;
use botanix_chainspec::constants::BOTANIX_TESTNET;
use btcserverlib::{database, database::version::UtxoVersion};
use ethers::{prelude::Provider, providers::Http};
use frost_secp256k1_tr as frost;

use crate::{
    it_info_print,
    suite::consensus::ConsensusIntegrationTestSuite,
    utils::{generate_blocks, get_gateway_address_with_retry},
};
const FROST_ID_PREFIX_LENGTH: usize = 6;

/// Helper function to shut down all sidechain processes (keeping only bitcoind)
async fn shutdown_sidechain_processes(suite: &mut ConsensusIntegrationTestSuite) {
    it_info_print!("Shutting down sidechain processes for CLI testing");

    // Shut down BTC server processes
    if let Some(btc_processes) = suite.local_context.btc_processes.as_mut() {
        for btc_process in btc_processes.iter_mut() {
            it_info_print!("Shutting down BTC server process");
            btc_process.destroy_all_async().await;
        }
    }

    // Shut down CometBFT processes
    if let Some(cometbft_processes) = suite.local_context.cometbft_processes.as_mut() {
        for cometbft_process in cometbft_processes.iter_mut() {
            it_info_print!("Shutting down CometBFT process");
            cometbft_process.destroy_all_async().await;
        }
    }

    // Shut down POA node processes
    if let Some(poa_processes) = suite.local_context.poa_processes.as_mut() {
        for poa_process in poa_processes.iter_mut() {
            it_info_print!("Shutting down POA node process");
            poa_process.destroy_all_async().await;
        }
    }

    // Shut down RPC node processes
    if let Some(rpc_processes) = suite.local_context.rpc_processes.as_mut() {
        for rpc_process in rpc_processes.iter_mut() {
            it_info_print!("Shutting down RPC node process");
            rpc_process.destroy_all_async().await;
        }
    }

    it_info_print!("All sidechain processes shut down - bitcoind remains running");
}

/// Helper function to run sweep CLI commands
fn run_sweep_cli_command(args: &[&str]) -> anyhow::Result<String, super::error::Error> {
    let output = Command::new("cargo")
        .args(&["run", "--package", "btc-server", "--bin", "sweep", "--"])
        .args(args)
        .output()
        .map_err(|e| {
            super::error::Error::TestVectorExport(format!("Failed to execute CLI command: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(super::error::Error::TestVectorExport(format!(
            "CLI command failed with status {}: {}",
            output.status, stderr
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.to_string())
}
#[allow(clippy::too_many_lines)]
pub async fn sweep_cli_e2e(
    suite: &mut ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), super::error::Error> {
    it_info_print!("Starting Sweep CLI E2E Test");

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

    // Everything above is more or less taken from test_frost_e2e.rs. The current state is:
    // - DKG complete
    // - There's an on-chain utxo corresponding to the gateway address, simulating a pegin utxo
    // Now, we will export and save the federation databases to temporary directories, so that
    // we can test the CLI commands, pointing them to the temporary databases and directories.

    // Phase 2 - Database Export & CLI Setup
    it_info_print!("Starting Phase 2: Database Export");

    // Export all federation databases to temporary directories
    let temp_db_dir = std::env::temp_dir().join(format!("sweep_cli_test_{}", uuid::Uuid::new_v4()));
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

    // Now, we can add the pegin utxo to the coordinator's database
    let coordinator_db = database::Db::open(&fed_db_paths[0]).expect("failed to open db");

    let utxo = database::Utxo::new(
        OutPoint::new(pegin_tx.compute_txid(), vout as u32),
        TxOut { value: amount, script_pubkey: btc_address.script_pubkey() },
        eth_destination.0.into(),
        Some(UtxoVersion::V1),
    );

    coordinator_db.store_utxos(&vec![&utxo]).expect("failed to store utxo");

    // validate the utxo is in the database
    let retrieved_utxo = coordinator_db.get_utxo(utxo.outpoint).expect("failed to get utxo");
    assert!(retrieved_utxo.as_ref().unwrap() == &utxo);

    it_info_print!(
        "Utxo stored in coordinator's database",
        format!("Utxo: {:?}", retrieved_utxo.unwrap())
    );

    // Release the database connection to free the lock before CLI commands
    drop(coordinator_db);

    // Shutdown sidechain processes. In such a recovery scenario we would
    // most likely halt all sidechain activity anyway.
    shutdown_sidechain_processes(suite).await;

    // Create CLI output directory and change working directory for the entire test process
    let cli_output_dir = temp_db_dir.join("cli_output");
    std::fs::create_dir_all(&cli_output_dir).map_err(|e| {
        super::error::Error::TestVectorExport(format!(
            "Failed to create CLI output directory: {}",
            e
        ))
    })?;

    // // TODO: Change the working directory so that generated files are saved in the temporary
    // directory std::env::set_current_dir(&cli_output_dir).map_err(|e| {
    //     super::error::Error::TestVectorExport(format!("Failed to change working directory: {}",
    // e)) })?;

    // it_info_print!("Changed working directory to", cli_output_dir.display());

    // Give a moment for processes to fully terminate and release locks
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Phase 3 - FROST Signing Workflow via CLI
    it_info_print!("Starting Phase 3: CLI Command Execution");

    // Set up a testnet output address for the sweep transaction
    let output_address = "tb1pzqgyfxcjfr43rdrnr29cms873jsvzdnln5mp5vuqz3tm9n7p68yqu2mpkd";
    let sat_per_vbyte = 10u64;

    // Step 1: coordinator-1-create-psbt
    it_info_print!("Executing coordinator-1-create-psbt command");
    let psbt_result = run_sweep_cli_command(&[
        "coordinator-1-create-psbt",
        "--db",
        &fed_db_paths[0].to_string_lossy(),
        "--output-address",
        output_address,
        "--sat-per-vbyte",
        &sat_per_vbyte.to_string(),
        "--testnet",
    ])?;
    it_info_print!("PSBT creation result", psbt_result);

    // Step 2: signer-1-generate-commitments (for multiple signers)
    // for each signer, including the coordinator, run the signer 1 command, pointing to that node's
    // db, and using their identifier
    for (index, db_path) in fed_db_paths.iter().enumerate() {
        // debugging: print the arguments being passed in
        it_info_print!(
            "Signer Round 1 arguments",
            format!("input-json: signing_package.json, db: {:?}, identifier: {}", db_path, index)
        );
        // debugging: print the current working directory
        it_info_print!("Current working directory", std::env::current_dir().unwrap().display());
        let signer_round1_result = run_sweep_cli_command(&[
            "signer-1-generate-commitments",
            "--input-json",
            "signing_package.json", // TODO: dont use hardcoded file name
            "--db",
            &db_path.to_string_lossy(),
            "--identifier",
            &index.to_string(),
        ])?;
        it_info_print!("Signer Round 1 result", signer_round1_result);
    }

    // Step 3: coordinator-2-collect-commitments
    it_info_print!("Executing coordinator-2-collect-commitments command");

    // TODO: use frost identifier prefix from sweep.rs instead of hardcoding the file names
    let collect_commitments_result = run_sweep_cli_command(&[
        "coordinator-2-collect-commitments",
        "--round1-responses", "round_1_response_acc59f.json,round_1_response_3427a0.json,round_1_response_0cdfdb.json,round_1_response_f7892e.json,round_1_response_8d10aa.json",
        "--min-signers", "3",
        "--output-json", "signing_package_round2.json",
        "--db", &fed_db_paths[0].to_string_lossy(),
    ])?;
    it_info_print!("Coordinator-2-Collect-Commitments result", collect_commitments_result);

    // Step 4: signer-2-generate-signatures (for selected signers)
    it_info_print!("Executing signer-2-generate-signatures command");
    for (index, db_path) in fed_db_paths.iter().enumerate() {
        // debugging: print the arguments being passed in
        it_info_print!(
            "Signer Round 2 arguments",
            format!("input-json: signing_package.json, db: {:?}, identifier: {}", db_path, index)
        );
        // debugging: print the current working directory
        it_info_print!("Current working directory", std::env::current_dir().unwrap().display());

        // TODO: reuse this frost id prefix from sweep.rs
        let frost_identifier = frost::Identifier::derive((index as u16).to_le_bytes().as_slice())
            .expect("valid frost identifier");
        let frost_id_prefix =
            hex::encode(frost_identifier.serialize())[..FROST_ID_PREFIX_LENGTH].to_string();

        let signer_round2_result = run_sweep_cli_command(&[
            "signer-2-generate-signatures",
            "--input-json",
            "signing_package_round2.json", // TODO: dont use hardcoded file name
            "--db",
            &db_path.to_string_lossy(),
            "--identifier",
            &index.to_string(),
            "--nonces-json",
            &format!("nonces_{}.json", frost_id_prefix),
        ])?;
        it_info_print!("Signer Round 2 result", signer_round2_result);
    }

    // Step 5: coordinator-3-finalize-transaction
    it_info_print!("Executing coordinator-3-finalize-transaction command");
    let finalize_transaction_result = run_sweep_cli_command(&[
        "coordinator-3-finalize-transaction",
        "--round2-responses", "round_2_response_acc59f.json,round_2_response_3427a0.json,round_2_response_0cdfdb.json,round_2_response_f7892e.json,round_2_response_8d10aa.json",
        "--min-signers", "3",
        "--output-file", "finalized_sweep_transaction_testnet.hex",
        "--db", &fed_db_paths[0].to_string_lossy(),
    ])?;
    it_info_print!("Coordinator-3-Finalize-Transaction result", finalize_transaction_result);

    // assert the file exists (in current working directory)
    let finalized_transaction_path =
        std::env::current_dir().unwrap().join("finalized_sweep_transaction_testnet.hex");
    assert!(finalized_transaction_path.exists());
    it_info_print!("Finalized transaction path", finalized_transaction_path.display());

    // Phase 4 - Bitcoin Integration & Validation

    // parse the transaction hex from the saved file
    let transaction_hex = std::fs::read_to_string(finalized_transaction_path)
        .expect("failed to read transaction hex");
    let transaction_hex = transaction_hex.trim(); // Remove any whitespace/newlines
    it_info_print!("Transaction hex", transaction_hex);

    // deserialize the hex string into a Bitcoin transaction
    let transaction: Transaction =
        deserialize_hex(transaction_hex).expect("failed to deserialize transaction hex");

    // broadcast the transaction to bitcoind
    let broadcast_txid =
        bitcoind_rpc.send_raw_transaction(&transaction).expect("failed to broadcast transaction");
    it_info_print!("Broadcast result", broadcast_txid);

    // assert that we get a valid txid back, meaning the transaction was successfully broadcasted
    assert_eq!(broadcast_txid.to_string().len(), 64, "txid should be 64 hex characters");

    // TODO: more validation on the transaction itself, in particular the fee rate

    // Phase 5 - Cleanup
    // TODO: Remove test files (mainly json files) created in current working directory

    // Cleanup: Remove temporary directory
    let _ = fs::remove_dir_all(&temp_db_dir);

    it_info_print!("Sweep CLI E2E Test completed successfully");

    Ok(())
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
