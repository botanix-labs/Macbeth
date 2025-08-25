use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    time::Duration,
};

use bitcoin::{hashes::Hash, merkle_tree::PartialMerkleTree, Amount};
use bitcoincore_rpc::RpcApi;
use botanix_authority_peg::{
    mint_validation::{BURN_TOPIC, MINT_TOPIC},
    peg_contract::{PeginData, PeginMeta, PeginMetaV0, PegoutData},
    utils::AmountExt,
};
use botanix_chainspec::constants::BOTANIX_TESTNET;
use btc_server_client::{BtcServerClient, GetFinalizedPegoutIdsRequest};
use ethers::{
    prelude::Provider,
    providers::{Http, Middleware},
    types::NameOrAddress,
};
use futures::StreamExt;
use reth_primitives::Address;
use tonic::transport::Channel;

use crate::{
    it_info_print,
    suite::consensus::{common::events::await_botanix_event, ConsensusIntegrationTestSuite},
    utils::{generate_blocks, get_gateway_address_with_retry},
};

pub async fn get_finalized_pegout_ids_from_peers(
    mut btc_servers: Vec<BtcServerClient<Channel>>,
) -> HashMap<usize, HashSet<Vec<u8>>> {
    let mut peers_finalized_pegout_ids: HashMap<usize, HashSet<Vec<u8>>> = HashMap::new();
    for (index, db_provider) in btc_servers.iter_mut().enumerate() {
        let mut pegout_ids_stream = db_provider
            .get_finalized_pegout_ids(GetFinalizedPegoutIdsRequest { chunk_size: 10 })
            .await
            .unwrap()
            .into_inner();
        while let Some(pegout_ids_chunk) = pegout_ids_stream.next().await {
            match pegout_ids_chunk {
                Ok(pegout_ids_chunk) => {
                    let _ = peers_finalized_pegout_ids
                        .entry(index)
                        .or_insert_with(HashSet::new)
                        .extend(
                            pegout_ids_chunk
                                .data
                                .into_iter()
                                .map(|finalized_pegout_id| finalized_pegout_id.id),
                        );
                }
                Err(_) => {
                    continue;
                }
            }
        }
    }
    peers_finalized_pegout_ids
}

#[allow(clippy::too_many_lines)]
pub async fn test_wallet_sync(
    suite: &mut ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), anyhow::Error> {
    // This test is for the happy path:
    // Create a pegout, sign, and broadcast
    // Then generate deeply confirmed blocks to finalize the pegout
    // Get finalized pegouts list from all peers
    // Wait for an epoch block and sync
    // Get finalized pegouts list from all peers again
    // Confirm the finalized pegouts list is the same as before

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
    let pegout_amount = Amount::from_btc(0.4).unwrap();
    let tx_receipt = mint_contract
        .burn(pegout_destination.clone(), pegout_data.clone(), pegout_amount.to_wei())
        .await
        .unwrap();
    it_info_print!("Pegout Tx Receipt: ", tx_receipt);

    // wait for the tx to be included in a botanix block
    await_botanix_event(&mut rx, *BURN_TOPIC).await;

    // wait until pegout has been broadcasted
    // can indirectly know if signing server has a tracked_tx
    let mut signer = suite
        .local_context
        .btc_server_clients
        .as_ref()
        .expect("btc_server_client")
        .first()
        .expect("btc server client")
        .clone();
    it_info_print!("Waiting for tracked tx");
    loop {
        let response = signer
            .get_tracked_txs(tonic::Request::new(btc_server_client::Empty {}))
            .await?
            .into_inner();
        if !response.tracked_txs.is_empty() {
            it_info_print!("Tracked tx found");
            break;
        }
        // sleep for 10s
        tokio::time::sleep(Duration::from_secs(10)).await;
    }

    // Reconnect to bitcoind. Occasionally the connection is lost after a long time or b/c of other
    // processes connecting
    let bitcoind_rpc = suite.global_context.bitcoind_rpc();
    // mine some btc blocks (needed for confirmed pegout)
    generate_blocks(&bitcoind_rpc, 10).await;

    // Finalized pegout ids are added during finalize_block():
    // deeply confirmed pegouts are moved to finalized pegouts.
    // This will occur first: before peers sync their wallets when their finalized pegouts list is
    // populated.
    it_info_print!("Waiting for block to be finalized");
    loop {
        // get all finalized pegout ids after block is finalized
        let peers_finalized_pegout_ids_after = get_finalized_pegout_ids_from_peers(
            suite.local_context.btc_server_clients.clone().unwrap(),
        )
        .await;

        let first_peer_finalized_pegout_ids =
            peers_finalized_pegout_ids_after.get(&0).cloned().unwrap_or_default();
        // wait until block has been finalized and finalized pegouts list is not empty
        if first_peer_finalized_pegout_ids.is_empty() {
            it_info_print!("finalized pegout ids empty");

            // sleep for 10s
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }
        it_info_print!("First peer finalized pegout ids", first_peer_finalized_pegout_ids);
        // assert that all peers have the same list
        for (_peer_id, peer_finalized_pegout_ids) in peers_finalized_pegout_ids_after {
            assert!(first_peer_finalized_pegout_ids.len() == peer_finalized_pegout_ids.len());
            assert!(first_peer_finalized_pegout_ids == peer_finalized_pegout_ids);
        }

        break;
    }
    it_info_print!("finalized pegout ids match before syncing");

    // Now that peers have finalized pegouts, we can sync wallets.
    // Wait for 60s to make sure we've reached the next epoch
    // and nodes have requested wallet state from peers.
    // TODO(scott): refactor to listen for wallet state request if possible
    tokio::time::sleep(Duration::from_secs(60)).await;
    it_info_print!("Waiting for wallets to sync");
    loop {
        // get all finalized pegout ids after the poa epoch
        let peers_finalized_pegout_ids_after = get_finalized_pegout_ids_from_peers(
            suite.local_context.btc_server_clients.clone().unwrap(),
        )
        .await;

        let first_peer_finalized_pegout_ids =
            peers_finalized_pegout_ids_after.get(&0).cloned().unwrap_or_default();
        // wait until wallets synced and finalized pegouts list is not empty
        if first_peer_finalized_pegout_ids.is_empty() {
            it_info_print!("finalized pegout ids empty");

            // sleep for 10s
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }
        it_info_print!("First peer finalized pegout ids", first_peer_finalized_pegout_ids);
        // assert that all peers have the same list
        for (_peer_id, peer_finalized_pegout_ids) in peers_finalized_pegout_ids_after {
            assert!(first_peer_finalized_pegout_ids.len() == peer_finalized_pegout_ids.len());
            assert!(first_peer_finalized_pegout_ids == peer_finalized_pegout_ids);
        }

        break;
    }

    it_info_print!("finalized pegout ids match after syncing");

    Ok(())
}
