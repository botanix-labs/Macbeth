use std::{str::FromStr, time::Duration};

use bitcoin::{hashes::Hash, merkle_tree::PartialMerkleTree, Amount};
use bitcoincore_rpc::RpcApi;
use botanix_authority_peg::{
    mint_validation::{BURN_TOPIC, MINT_TOPIC},
    peg_contract::{PeginData, PeginMeta, PeginMetaV0, PegoutData},
    utils::AmountExt,
};
use botanix_chainspec::constants::BOTANIX_TESTNET;
use ethers::{
    prelude::Provider,
    providers::{Http, Middleware},
    types::NameOrAddress,
};
use reth_primitives::Address;

use crate::{
    it_info_print,
    suite::consensus::{common::events::await_botanix_event, ConsensusIntegrationTestSuite},
    utils::{generate_blocks, get_gateway_address_with_retry},
};

// Imports for test vector export functionality
use btc_server_client;
use btcserverlib::database;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[allow(clippy::too_many_lines)]
pub async fn frost_e2e_stable(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), super::error::Error> {
    let pegin_conf_depth = BOTANIX_TESTNET.bitcoin_checkpoint_confirmation_depth;
    it_info_print!("Pegin Confirmation Depth", pegin_conf_depth);

    // TEST VECTOR EXPORT - Check environment variable and export if requested
    // This is the perfect spot because:
    // 1. DKG has already completed (POA nodes and BTC servers are running)
    // 2. No UTXOs or transactions have been created yet
    // 3. We have access to the suite context with all the database paths
    if let Ok(export_path) = std::env::var("EXPORT_TEST_VECTOR") {
        it_info_print!("Exporting test vector to: {}", export_path);
        export_test_vector_db(suite, &export_path).await?;
        it_info_print!("Test vector export completed");
    }

    // Set up regtest connection
    // config is hardcoded to only work with regtest
    let bitcoind_rpc = suite.global_context.bitcoind_rpc();
    tokio::time::sleep(Duration::from_secs(5)).await;

    let test_fed_members = suite
        .local_context
        .poa_nodes
        .as_ref()
        .expect("test federation member configurations")
        .clone();
    let mut rx = suite.local_context.poa_notification.as_ref().expect("poa notifs").subscribe();

    // generate mint contract test instances
    let mut mint_contract_instances = Vec::new();
    for (index, _) in test_fed_members.iter() {
        let botanix_eth_client =
            test_fed_members.get(index).cloned().unwrap().botanix_eth_client.clone();
        mint_contract_instances.push(botanix_eth_client);
    }

    // Set up dummy eth address
    let eth_destination = ethers::core::types::Address::random();

    // Provider to one of the federation members
    let provider = Provider::<Http>::try_from(format!(
        "http://localhost:{}",
        test_fed_members.get(&0).unwrap().rpc_port
    ))
    .expect("could not instantiate HTTP Provider");

    // get gateway address
    let gateway_address_response =
        get_gateway_address_with_retry(provider.clone(), eth_destination.0.into(), 3).await?;
    it_info_print!("Gateway Address Response", gateway_address_response);

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

    let eth_account = Address::from_slice(eth_destination.as_bytes());
    let (vout, pegin_output) = pegin_tx
        .output
        .iter()
        .enumerate()
        .find(|(_, o)| o.script_pubkey == btc_address.script_pubkey())
        .unwrap();
    let amount = pegin_output.value.to_wei();
    it_info_print!("Btc Amount", amount);

    // get block headers
    // first we need the block hash of the block with the conf'd pegin tx
    let conf_hash = tx_res.info.blockhash.expect("pegin confirmed");
    let tip = bitcoind_rpc.get_best_block_hash().unwrap();
    it_info_print!("Bitcoin Chain Tip", tip);
    let tip_header = bitcoind_rpc.get_block_header(&tip).expect("valid block header");
    // We will collect all the headers all the way up to the tip which is not needed, but allowed.
    // In theory, we only need to collect headers from the block our pegin is in, to the finalized
    // block (the one in the mainchain commitment).
    let mut headers = vec![];
    let mut cursor = tip_header;
    let mut stopgap = 200; // just to make sure we don't infinite loop until genesis
    loop {
        stopgap -= 1;
        if stopgap == 0 || cursor.prev_blockhash == bitcoin::BlockHash::all_zeros() {
            panic!("confirmation block not found...");
        }

        headers.push(cursor);
        if cursor.block_hash() == conf_hash {
            break;
        }
        cursor = bitcoind_rpc.get_block_header(&cursor.prev_blockhash).unwrap();
    }
    headers.reverse();
    it_info_print!("Number of pegin_headers:", headers.len());

    let conf_block_info = bitcoind_rpc.get_block_info(&conf_hash).expect("valid txids");
    it_info_print!("Block info", conf_block_info);
    let pmt = PartialMerkleTree::from_txids(&conf_block_info.tx, &[false, true]);

    // create pegin meta
    let bitcoin_block_height = conf_block_info.height;
    let meta = PeginMeta::V0(PeginMetaV0 {
        version: 0,
        outpoint: bitcoin::OutPoint::new(pegin_tx.compute_txid(), vout as u32),
        address: eth_account,
        aggregate_publickey: secp256k1::PublicKey::from_str(
            gateway_address_response.aggregate_public_key.as_str(),
        )
        .expect("valid public key"),
        tx: pegin_tx.clone(),
        merkle_proof: pmt,
        block_headers: headers,
    });

    // validate the pegin data first offchain before submitting
    let pegin_data = PeginData {
        account: Address::from_slice(eth_destination.as_bytes()),
        amount,
        bitcoin_block_height: bitcoin_block_height as u32,
        meta: vec![meta.clone()],
    };
    let checkpoint = {
        let tip = bitcoind_rpc.get_block_count().unwrap();
        let height = tip - pegin_conf_depth as u64;
        let hash = bitcoind_rpc.get_block_hash(height).unwrap();
        (bitcoind_rpc.get_block_header(&hash).unwrap(), height as u32)
    };
    pegin_data
        .validate(
            &checkpoint,
            &secp256k1::PublicKey::from_str(gateway_address_response.aggregate_public_key.as_str())
                .unwrap(),
        )
        .expect("pegin data should be valid!");
    it_info_print!("Pegindata successfully validated");

    // send the pegin transactions to all fed members
    it_info_print!(
        "Sending pegin tx: block headers=",
        meta.block_headers().iter().map(|h| h.block_hash()).collect::<Vec<_>>()
    );
    let serialized_pegin_meta = meta.serialize().unwrap();
    it_info_print!("Serialized pegin meta: ", hex::encode(serialized_pegin_meta.clone()));
    let mint_contract = mint_contract_instances
        .first()
        .cloned()
        .unwrap()
        .expect("Botanix Client must be initialized");
    let metadata = ethers::core::types::Bytes::from(serialized_pegin_meta.clone());
    let tx_receipt = mint_contract
        .mint(
            eth_destination.clone(),
            amount,
            bitcoin_block_height as u32,
            metadata,
            ethers::core::types::Address::random(),
        )
        .await
        .unwrap();
    it_info_print!("Mint Tx Receipt ", tx_receipt);

    // wait for a few blocks to make sure the tx got included and mined
    it_info_print!("Waiting for botanix event after mint call");
    await_botanix_event(&mut rx, *MINT_TOPIC).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // make sure we have received the botanix btc on botanix
    let eth_address = NameOrAddress::from_str(&eth_account.to_string()).unwrap();
    let eth_address_balance = provider.get_balance(eth_address, None).await.unwrap();
    assert!(!eth_address_balance.is_zero());

    // Generate and send pegout tx
    // bitcoin address
    let pegout_destination =
        ethers::core::types::Bytes::from(btc_address.to_string().as_bytes().to_vec());
    // set pegout version
    let pegout_data = ethers::core::types::Bytes::from(vec![PegoutData::version()]);
    let pegout_amount = Amount::from_btc(0.5).unwrap();
    let tx_receipt =
        mint_contract.burn(pegout_destination, pegout_data, pegout_amount.to_wei()).await.unwrap();
    it_info_print!("Pegout Tx Receipt: ", tx_receipt);

    // wait for the tx to be included in a botanix block
    await_botanix_event(&mut rx, *BURN_TOPIC).await;

    // sleep for a few more seconds
    tokio::time::sleep(Duration::from_secs(50)).await;

    // Reconnect to bitcoind. Occasionally the connection is lost after a long time or b/c of other
    // processes connecting
    let bitcoind_rpc = suite.global_context.bitcoind_rpc();
    // mine some btc blocks (needed for confirmed pegout)
    generate_blocks(&bitcoind_rpc, 1).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Retrieve the last block
    let tip_hash = bitcoind_rpc.get_best_block_hash().expect("valid block hash");
    let tip_block = bitcoind_rpc.get_block(&tip_hash).expect("valid block");
    // there should be 2 transaction one of which is the pegout the other is coinbase
    assert_eq!(tip_block.txdata.len(), 2);
    let pegout_tx = tip_block.txdata.get(1).unwrap();
    it_info_print!("Pegout tx: ", pegout_tx);

    assert_eq!(pegout_tx.input.len(), 1);
    assert_eq!(pegout_tx.input[0].previous_output.txid, pegin_tx.compute_txid());
    assert_eq!(pegout_tx.input[0].previous_output.vout, vout as u32);
    assert_eq!(pegout_tx.output.len(), 2);
    // One of the values here should be the pegout address
    let mut match_found = false;
    for output in pegout_tx.output.iter() {
        let pegout_address = output.script_pubkey.clone();
        let address_spk = btc_address.script_pubkey();
        match_found = pegout_address == address_spk;
        if match_found {
            break;
        }
    }
    assert!(match_found);
    // TODO We could do a precise amounts check here
    assert!(pegout_tx.output[1].value > Amount::from_sat(0));

    // Verify the fee is exactly what we expect
    let total_input_value = pegin_tx.output[vout].value;
    it_info_print!("Total input value: ", total_input_value);
    let total_output_value = pegout_tx.output[0].value + pegout_tx.output[1].value;
    it_info_print!("Total output value: ", total_output_value);
    let actual_fee = total_input_value - total_output_value;
    it_info_print!("Actual fee: ", actual_fee);
    let weight = pegout_tx.weight();
    it_info_print!("Weight: ", weight);
    let expected_fee_rate = 1250; // 1250 sat/kwu is equivalent to 0.00005 sat/byte, which is the fallbackfee set in bitcoin conf
    let expected_fee = (expected_fee_rate * weight.to_wu() + 999) / 1000; // Rounding up to nearest sat
    it_info_print!("Expected fee: ", expected_fee);
    assert_eq!(actual_fee, Amount::from_sat(expected_fee));

    // Verify witness signatures are 64 bytes (Taproot signature size when using SIGHASH_DEFAULT)
    for input in pegout_tx.input.iter() {
        let witness_item = &input.witness[0];
        it_info_print!("Input witness (signature) length:", witness_item.len());
        assert_eq!(witness_item.len(), 64);
    }

    Ok(())
}

// TEST VECTOR EXPORT FUNCTIONS

/// Metadata to save alongside the database export
#[derive(Serialize, Deserialize)]
struct TestVectorMetadata {
    change_address: String,
    network: String,
    aggregate_public_key: String,
    min_signers: u16,
    max_signers: u16,
    export_timestamp: u64,
}
/// Main export function - copies database and extracts metadata
async fn export_test_vector_db(
    suite: &ConsensusIntegrationTestSuite,
    output_dir: &str,
) -> Result<(), super::error::Error> {
    // Create output directory
    fs::create_dir_all(output_dir).map_err(|e| {
        super::error::Error::TestVectorExport(format!("Failed to create output directory: {}", e))
    })?;

    // Get the first BTC server's database path
    let source_db_path = get_btc_server_db_path(suite)?;

    // Copy the entire sled database directory
    let target_db_path = format!("{}/test_vector_db", output_dir);
    copy_dir_recursively(&source_db_path, Path::new(&target_db_path))?;

    // Extract change address and other metadata
    let change_address = extract_change_address_from_db(&source_db_path)?;

    // Get aggregate public key
    let aggregate_public_key = extract_aggregate_public_key(suite).await?;

    // Create metadata
    let metadata = TestVectorMetadata {
        change_address,
        network: "regtest".to_string(),
        aggregate_public_key,
        min_signers: suite.global_context.min_signers,
        max_signers: suite.global_context.max_signers,
        export_timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| {
                super::error::Error::TestVectorExport(format!("Failed to get timestamp: {}", e))
            })?
            .as_secs(),
    };

    // Save metadata
    let metadata_json = serde_json::to_string_pretty(&metadata).map_err(|e| {
        super::error::Error::TestVectorExport(format!("Failed to serialize metadata: {}", e))
    })?;
    fs::write(format!("{}/metadata.json", output_dir), metadata_json).map_err(|e| {
        super::error::Error::TestVectorExport(format!("Failed to write metadata file: {}", e))
    })?;

    it_info_print!("CHANGE_ADDRESS: {}", metadata.change_address);
    it_info_print!("AGGREGATE_PUBLIC_KEY: {}", metadata.aggregate_public_key);
    it_info_print!("DATABASE_EXPORTED_TO: {}", target_db_path);

    Ok(())
}
/// Get the database path from the first BTC server process
fn get_btc_server_db_path(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<std::path::PathBuf, super::error::Error> {
    let btc_processes = suite.local_context.btc_processes.as_ref().ok_or_else(|| {
        super::error::Error::TestVectorExport("No BTC server processes found".to_string())
    })?;

    let first_process = btc_processes.first().ok_or_else(|| {
        super::error::Error::TestVectorExport("No BTC server processes in list".to_string())
    })?;

    Ok(first_process.db_path.clone())
}
/// Extract change address by deriving it from the public key package in the database
fn extract_change_address_from_db(
    db_path: &std::path::Path,
) -> Result<String, super::error::Error> {
    use bitcoin::{secp256k1::Secp256k1, Network};
    use btcserverlib::wallet::util::VerifyingKeyExt;

    // Open the database
    let db = database::Db::open(db_path).map_err(|e| {
        super::error::Error::TestVectorExport(format!("Failed to open database: {}", e))
    })?;

    // Get the public key package
    let pk_package = db
        .get_public_key_package()
        .map_err(|e| {
            super::error::Error::TestVectorExport(format!(
                "Failed to get public key package: {}",
                e
            ))
        })?
        .ok_or_else(|| {
            super::error::Error::TestVectorExport(
                "No public key package found in database".to_string(),
            )
        })?;

    // Convert FROST verifying key to secp256k1 public key
    let verifying_key = pk_package.verifying_key();
    let secp_pubkey = verifying_key.to_secp_pk().map_err(|e| {
        super::error::Error::TestVectorExport(format!(
            "Failed to convert FROST key to secp256k1: {}",
            e
        ))
    })?;

    // Generate change address (taproot P2TR without eth address tweak)
    let secp = Secp256k1::new();
    let script = bitcoin::ScriptBuf::new_p2tr(&secp, secp_pubkey.x_only_public_key().0, None);
    let address = bitcoin::Address::from_script(&script, Network::Regtest).map_err(|e| {
        super::error::Error::TestVectorExport(format!(
            "Failed to create address from script: {}",
            e
        ))
    })?;

    Ok(address.to_string())
}
/// Extract aggregate public key from the test suite context
async fn extract_aggregate_public_key(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<String, super::error::Error> {
    // Get BTC server clients to query for the public key
    let btc_processes = suite.local_context.btc_processes.as_ref().ok_or_else(|| {
        super::error::Error::TestVectorExport("No BTC server processes found".to_string())
    })?;

    let first_process = btc_processes.first().ok_or_else(|| {
        super::error::Error::TestVectorExport("No BTC server processes in list".to_string())
    })?;

    // Connect to the first BTC server and get its public key
    let client = btc_server_client::BtcServerClient::connect(format!(
        "http://localhost:{}",
        first_process.btc_server_port
    ))
    .await
    .map_err(|e| {
        super::error::Error::TestVectorExport(format!("Failed to connect to BTC server: {}", e))
    })?;

    let mut client = client;
    let response = client
        .get_public_key(tonic::Request::new(btc_server_client::Empty {}))
        .await
        .map_err(|e| {
            super::error::Error::TestVectorExport(format!(
                "Failed to get public key from BTC server: {}",
                e
            ))
        })?;

    Ok(response.into_inner().publickey)
}
/// Recursively copy a directory
fn copy_dir_recursively(src: &Path, dst: &Path) -> Result<(), super::error::Error> {
    fs::create_dir_all(dst).map_err(|e| {
        super::error::Error::TestVectorExport(format!(
            "Failed to create directory {}: {}",
            dst.display(),
            e
        ))
    })?;

    for entry in fs::read_dir(src).map_err(|e| {
        super::error::Error::TestVectorExport(format!(
            "Failed to read directory {}: {}",
            src.display(),
            e
        ))
    })? {
        let entry = entry.map_err(|e| {
            super::error::Error::TestVectorExport(format!("Failed to read directory entry: {}", e))
        })?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursively(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path).map_err(|e| {
                super::error::Error::TestVectorExport(format!(
                    "Failed to copy file {} to {}: {}",
                    src_path.display(),
                    dst_path.display(),
                    e
                ))
            })?;
        }
    }

    Ok(())
}
