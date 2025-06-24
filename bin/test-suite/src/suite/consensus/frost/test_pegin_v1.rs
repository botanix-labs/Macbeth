use std::{str::FromStr, time::Duration};

use bitcoin::{merkle_tree::PartialMerkleTree, Amount};
use bitcoincore_rpc::RpcApi;
use ethers::{
    prelude::Provider,
    providers::{Http, Middleware},
    types::NameOrAddress,
};
use reth_primitives::{
    botanix::{
        mint_validation::MINT_TOPIC,
        peg_contract::{PeginData, PeginMeta, PeginMetaV0, PeginMetaV1},
        utils::AmountExt,
    },
    revm_primitives::FixedBytes,
};

use reth_chainspec::BOTANIX_TESTNET;
use reth_primitives::Address;
use serde_json::json;

use crate::{
    it_info_print,
    suite::consensus::{
        common::events::{await_botanix_event, BlockWithEDH},
        ConsensusIntegrationTestSuite,
    },
    utils::{generate_blocks, get_gateway_address_with_retry},
};

pub async fn test_pegin_v1(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), super::error::Error> {
    let pegin_conf_depth = BOTANIX_TESTNET.bitcoin_checkpoint_confirmation_depth;
    it_info_print!("Pegin Confirmation Depth", pegin_conf_depth);

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
    let mut clients = Vec::new();
    for (index, _) in test_fed_members.iter() {
        let botanix_eth_client =
            test_fed_members.get(index).cloned().unwrap().botanix_eth_client.clone();
        clients.push(botanix_eth_client);
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
    // Generate some blocks to confirm it
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

    // Create a Pegin V1 transaction
    // get the latest L2 block so we can use the btc checkpoint in the EDH
    let mut latest_block_with_edh;
    let mut btc_checkpoint_hash;
    let mut checkpoint_block_height;
    let conf_hash = tx_res.info.blockhash.expect("pegin confirmed");
    let conf_block_info = bitcoind_rpc.get_block_info(&conf_hash).expect("valid block info");
    let bitcoin_block_height = conf_block_info.height;
    it_info_print!("Pegin confirmed at Bitcoin height", bitcoin_block_height);

    // Retry until L2 checkpoint covers our pegin block
    let mut retry_count = 0;
    loop {
        let latest_block_result = provider
            .request::<Vec<serde_json::Value>, BlockWithEDH>(
                "eth_getBlockByNumber",
                vec![json!("latest"), json!(false), json!(true)],
            )
            .await;
        latest_block_with_edh = latest_block_result.expect("valid block with edh");
        let btc_checkpoint = latest_block_with_edh.extra_data_header.bitcoin_block_hash;
        btc_checkpoint_hash =
            bitcoin::BlockHash::from_str(btc_checkpoint.as_str()).expect("valid hash");
        checkpoint_block_height =
            bitcoind_rpc.get_block_info(&btc_checkpoint_hash).expect("valid block info").height;

        it_info_print!("L2 checkpoint height", checkpoint_block_height);

        if checkpoint_block_height >= bitcoin_block_height {
            it_info_print!("L2 checkpoint now covers pegin block");
            break;
        }

        retry_count += 1;
        if retry_count > 30 {
            panic!("L2 checkpoint did not advance to cover pegin block after 30 retries");
        }

        it_info_print!("Waiting for L2 to advance checkpoint...", retry_count);
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    // The L2 block hash from eth_getBlockByNumber becomes the ref_block_hash for pegin v1
    let latest_block_hash = latest_block_with_edh.hash;
    it_info_print!("BTC checkpoint hash:", btc_checkpoint_hash);
    it_info_print!("L2 ref_block_hash:", latest_block_hash);

    // get the block hash of the block with the confirmed pegin tx
    let checkpoint_header =
        bitcoind_rpc.get_block_header(&btc_checkpoint_hash).expect("valid header");

    it_info_print!("Block info", conf_block_info);

    let pmt = PartialMerkleTree::from_txids(&conf_block_info.tx, &[false, true]);

    // create pegin meta
    // Ensure the pegin confirmation block is not newer than the BTC checkpoint.
    assert!(
        bitcoin_block_height <= checkpoint_block_height,
        "Pegin confirmation block height ({}) is greater than BTC checkpoint block height ({}). The pegin should be confirmed before or at the checkpoint.",
        bitcoin_block_height,
        checkpoint_block_height
    );

    let mut bitcoin_headers_chain: Vec<bitcoin::block::Header> = Vec::new();

    let log_msg_start = format!("Starting to build bitcoin_headers_chain. Initial conf_hash: {}, btc_checkpoint_hash: {}, bitcoin_block_height: {}, checkpoint_block_height: {}", conf_hash, btc_checkpoint_hash, bitcoin_block_height, checkpoint_block_height);
    it_info_print!(&log_msg_start);

    // Build headers from pegin confirmation block towards checkpoint block
    let mut current_block_iter_hash = conf_hash;
    for i in 0..=(checkpoint_block_height - bitcoin_block_height) {
        let effective_height = bitcoin_block_height + i;
        let log_msg_fetch = format!("Fetching header for block hash (current_block_iter_hash): {} (iteration {} - effective height {})", current_block_iter_hash, i, effective_height);
        it_info_print!(&log_msg_fetch);

        let header_to_add =
            bitcoind_rpc.get_block_header(&current_block_iter_hash).expect(&format!(
                "Failed to get block header for hash {} at effective height {}",
                current_block_iter_hash, effective_height
            ));

        let log_msg_fetched = format!(
            "Fetched header: hash {}, prev_blockhash {}",
            header_to_add.block_hash(),
            header_to_add.prev_blockhash
        );
        it_info_print!(&log_msg_fetched);

        // Verify hash-chain continuity starting from the second iteration
        if i > 0 {
            let last_header =
                bitcoin_headers_chain.last().expect("bitcoin_headers_chain should not be empty");
            assert_eq!(
                header_to_add.prev_blockhash,
                last_header.block_hash(),
                "Chain discontinuity detected: header at height {} has prev_blockhash {} but previous header hash is {}",
                effective_height,
                header_to_add.prev_blockhash,
                last_header.block_hash()
            );
        }

        bitcoin_headers_chain.push(header_to_add);

        if i < (checkpoint_block_height - bitcoin_block_height) {
            // If not the last header, get the next block hash for next iteration
            let next_block_height = effective_height + 1;
            current_block_iter_hash = bitcoind_rpc
                .get_block_hash(next_block_height as u64)
                .expect(&format!("Failed to get block hash for height {}", next_block_height));
        } else {
            // This was the last header fetched, it should correspond to the checkpoint block
            assert_eq!(
                header_to_add.block_hash(),
                btc_checkpoint_hash,
                "The last header fetched in the chain does not match the expected btc_checkpoint_hash."
            );
        }
    }

    let final_chain_hashes_str =
        format!("{:?}", bitcoin_headers_chain.iter().map(|h| h.block_hash()).collect::<Vec<_>>());
    let log_msg_final = format!(
        "Final bitcoin_headers_chain ({} headers): {}",
        bitcoin_headers_chain.len(),
        final_chain_hashes_str
    );
    it_info_print!(&log_msg_final);

    let meta = PeginMeta::V1(PeginMetaV1 {
        inner: PeginMetaV0 {
            version: 1,
            outpoint: bitcoin::OutPoint::new(pegin_tx.compute_txid(), vout as u32),
            address: eth_account,
            aggregate_publickey: secp256k1::PublicKey::from_str(
                gateway_address_response.aggregate_public_key.as_str(),
            )
            .expect("valid public key"),
            tx: pegin_tx.clone(),
            merkle_proof: pmt,
            block_headers: bitcoin_headers_chain,
        },
        ref_block_hash: FixedBytes::<32>::from_str(&latest_block_hash.as_str())
            .expect("valid hash"),
    });

    // validate the pegin data first offchain before submitting
    let pegin_data = PeginData {
        account: Address::from_slice(eth_destination.as_bytes()),
        amount,
        bitcoin_block_height: bitcoin_block_height as u32,
        meta: vec![meta.clone()],
    };
    let checkpoint = { (checkpoint_header, checkpoint_block_height as u32) };
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
    let metadata = ethers::core::types::Bytes::from(serialized_pegin_meta.clone());
    let client = clients.first().cloned().unwrap().expect("Botanix Client must be initialized");
    let tx_receipt = client
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

    Ok(())
}
