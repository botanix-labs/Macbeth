use std::{str::FromStr, time::Duration};

use bitcoin::{hashes::Hash, merkle_tree::PartialMerkleTree, Amount};
use bitcoincore_rpc::RpcApi;
use ethers::{prelude::Provider, providers::Http};
use reth_botanix_lib::{peg_contract::PeginMeta, utils::AmountExt};
use reth_btc_wallet::address::EthAddress;

use reth_primitives::Address;

use crate::{
    it_info_print,
    suite::consensus::{
        common::events::{GatewayAddressResponse, BITCOIND_WALLET_NAME},
        ConsensusIntegrationTestSuite,
    },
};

#[allow(clippy::too_many_lines)]
pub async fn invalid_pegin(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), super::error::InvalidTransactionError> {
    // Set up regtest connection
    // config is hardcoded to only work with regtest
    let bitcoind_rpc = suite.global_context.bitcoind_rpc();

    // Load up the bitcoin wallet and generate some blocks
    for wallet in bitcoind_rpc.list_wallets().unwrap() {
        it_info_print!("#UNLOADING WALLET?", &wallet);
        let _ = bitcoind_rpc.unload_wallet(Some(&wallet));
    }
    let create_res = bitcoind_rpc.create_wallet(BITCOIND_WALLET_NAME, None, None, None, None);
    if create_res.is_err() {
        // wallet already exists
        // load wallet
        let _ = bitcoind_rpc.load_wallet(BITCOIND_WALLET_NAME);
    }
    let address =
        bitcoind_rpc.get_new_address(None, None).expect("get new address").assume_checked();
    // generate > 100 blocks so coinbase utxos can be spent from the wallet
    bitcoind_rpc.generate_to_address(101, &address).expect("generate to address");
    tokio::time::sleep(Duration::from_secs(5)).await;

    let test_fed_members = suite
        .local_context
        .poa_nodes
        .as_ref()
        .expect("test federation member configurations")
        .clone();

    // subscribe to notifications so channel stays open
    let _rx = suite.local_context.poa_notification.as_ref().expect("poa notifs").subscribe();

    // generate mint contract test instances
    let mut mint_contract_instances = Vec::new();
    for (index, _) in test_fed_members.iter() {
        let botanix_eth_client =
            test_fed_members.get(index).cloned().unwrap().create_botanix_eth_client().await;
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
    let gateway_address_response = provider
        .request::<Vec<String>, GatewayAddressResponse>(
            "eth_getGatewayAddress",
            vec![hex::encode(eth_destination.0)],
        )
        .await
        .expect("should get gateway address");

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
    bitcoind_rpc
        .generate_to_address(
            2 + reth_primitives::constants::MAINNET_PEGIN_CONFIRMATION_DEPTH as u64,
            &address,
        )
        .expect("generate to address");
    tokio::time::sleep(Duration::from_secs(5)).await;

    // retrieve the transaction
    let tx_res = bitcoind_rpc.get_transaction(&pegin_txid, None).expect("valid tx");
    assert!(tx_res.info.confirmations > 1);
    let pegin_tx = tx_res.transaction().expect("valid tx");
    it_info_print!("Bitcoin pegin Tx", pegin_tx);
    it_info_print!("Gateway Data", gateway_address_response);
    it_info_print!("Gateway Data Pub key", gateway_address_response.aggregate_public_key);

    let eth_account = Address::from_slice(eth_destination.as_slice());
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
    it_info_print!("Number of pegin_headers: {}", headers.len());

    // create partial merkle tree
    let conf_hash = tx_res.info.blockhash.expect("pegin confirmed");
    let conf_block_info = bitcoind_rpc.get_block_info(&conf_hash).expect("valid txids");
    it_info_print!("Block info", conf_block_info);
    let pmt = PartialMerkleTree::from_txids(&conf_block_info.tx, &[false, true]);

    // create invalid pegin meta with empty headers list
    let bitcoin_block_height = conf_block_info.height;
    let meta = PeginMeta {
        version: 0,
        outpoint: bitcoin::OutPoint::new(pegin_tx.txid(), vout as u32),
        address: eth_account.clone(),
        aggregate_publickey: bitcoin::secp256k1::PublicKey::from_str(
            gateway_address_response.aggregate_public_key.as_str(),
        )
        .expect("valid public key"),
        tx: pegin_tx.clone(),
        merkle_proof: pmt,
        block_headers: vec![],
    };

    // send the pegin transactions to all fed members
    let serialized_pegin_meta = meta.serialize();
    it_info_print!("Serialized pegin meta: ", hex::encode(serialized_pegin_meta.clone()));
    let botanix_eth_client = mint_contract_instances.first().cloned().unwrap();
    let metadata = ethers::core::types::Bytes::from(serialized_pegin_meta.clone());

    // pegin address balance before pegin
    let eth_pegin_address = eth_account.to_string();
    let pegin_address_initial_balance =
        botanix_eth_client.get_botanix_balance(eth_pegin_address.as_str()).await.unwrap();
    it_info_print!("Initial pegin address balance", pegin_address_initial_balance);

    // nonce before pegin
    let sender_address = botanix_eth_client.get_sender_address();
    it_info_print!("Sender address", sender_address);
    let nonce_before = botanix_eth_client.get_nonce(sender_address.clone()).await.unwrap();
    it_info_print!("Nonce before pegin", nonce_before);

    it_info_print!("Sending invalid pegin transaction to mint contract");
    let tx_receipt = botanix_eth_client
        .mint(
            eth_destination.clone(),
            amount,
            bitcoin_block_height as u32,
            metadata,
            ethers::core::types::Address::random(),
        )
        .await
        .unwrap()
        .unwrap();

    // status should be 0 (failure)
    it_info_print!("Pegin Tx Receipt", tx_receipt);
    assert!(tx_receipt.status.unwrap().is_zero());

    // pegin address balance after pegin
    let pegin_address_final_balance =
        botanix_eth_client.get_botanix_balance(eth_pegin_address.as_str()).await.unwrap();
    it_info_print!("Final pegin address balance", pegin_address_final_balance);

    assert_eq!(pegin_address_initial_balance, pegin_address_final_balance);

    // nonce after pegin
    let nonce_after = botanix_eth_client.get_nonce(sender_address).await.unwrap();
    it_info_print!("Nonce after pegin", nonce_after);

    assert!(nonce_after > nonce_before);

    Ok(())
}
