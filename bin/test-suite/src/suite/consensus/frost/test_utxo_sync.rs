use bitcoin::Address;

use botanix_authority_edh::extra_data_header::ExtraDataHeader;
use ethers::types::H256;
use hex::{self, encode as hex_encode};
use std::{collections::HashSet, str::FromStr, time::Duration};

use crate::{
    it_info_print,
    suite::consensus::{
        common::{events::SEND_AMOUNT, poa_node::Notifications},
        frost::test_utxo_commitment::Pegins,
        ConsensusIntegrationTestSuite,
    },
    utils::{get_checkpoint_block_hash, send_pegins_notifications},
};

#[allow(clippy::too_many_lines)]
pub async fn utxo_sync(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), super::error::Error> {
    it_info_print!("Running block builder test...");
    let test_fed_members = suite
        .local_context
        .poa_nodes
        .as_ref()
        .expect("test federation member configurations")
        .clone();
    let mut rx = suite.local_context.poa_notification.as_ref().expect("poa notifs").subscribe();

    let mut btc_server_clients =
        suite.local_context.btc_server_clients.clone().expect("btc server clients");

    // Create a fake utxo -- this utxo doesn't need to exist on regtest
    // We are just testing the UTXO sync mechanism. All nodes should have the same UTXOs before
    // attempting to build or verify a block
    let mut pegins = Pegins::new();
    let n = 5;
    for _ in 0..n {
        // Copied from test_utxo_commitment.rs
        let eth_address = ethers::core::types::Address::random();
        pegins.eth_addresses.push(eth_address);
        pegins.txids.push(rand::random::<[u8; 32]>());
        let pk = btc_server_clients[0]
            .get_gateway_address(tonic::Request::new(client::GetGatewayAddressRequest {
                eth_address: hex_encode(eth_address),
            }))
            .await
            .expect("get response")
            .into_inner();
        let btc_address =
            Address::from_str(&pk.gateway_address).expect("valid address").assume_checked();
        pegins.btc_addresses.push(btc_address);
    }

    // get the checkpoint blockhash
    let bitcoind = suite.global_context.bitcoind_rpc();
    let checkpoint_block_hash = get_checkpoint_block_hash(&bitcoind)?;

    for c in btc_server_clients.iter_mut() {
        send_pegins_notifications(
            c,
            checkpoint_block_hash.clone(),
            pegins.txids.iter().map(|a| a.to_vec()).collect(),
            pegins.eth_addresses.iter().map(hex::encode).collect(),
            pegins.btc_addresses.clone(),
            vec![100_000_000; n],
        )
        .await?;
    }

    // find out who is in turn
    let inturn_member_index = 0;

    // assign targeted fed member
    let targeted_fed_member = test_fed_members.get(&(inturn_member_index as u16)).cloned().unwrap();

    // create a minting contract instance
    let botanix_eth_client =
        targeted_fed_member.botanix_eth_client.clone().expect("Botanix Client must be initialized");

    // send eoa messages to the node at selected index
    it_info_print!("Sending eoa transaction...");
    let tx_receipt = botanix_eth_client
        .send_eoa(ethers::core::types::Address::random(), SEND_AMOUNT)
        .await
        .unwrap()
        .unwrap();
    it_info_print!("Eoa tx receipt hash: {:?}", tx_receipt.transaction_hash);

    let mut poa_eth_clients = suite.local_context.poa_eth_providers.clone().unwrap();
    // remove the syncing clients
    for _syncing_node in 0..suite.global_context.syncing_instances as usize {
        poa_eth_clients.pop().unwrap();
    }

    // wait for canonical chain updates reported by the node, then send new tx
    let mut tx_hashes_set: HashSet<u16> = HashSet::new();
    while let Ok(notification) = rx.recv().await {
        if let Notifications::CanonState(canon_state_notification) = notification {
            it_info_print!(
                "Received payload from engine index",
                canon_state_notification.engine_index
            );
            it_info_print!(
                "Received block number from engine = {:?}",
                canon_state_notification.block.number.map(|n| n.as_u64())
            );

            // read all tx hashes from the block receipts
            let block_receipt_hashes = canon_state_notification
                .tx_receipts
                .iter()
                .map(|r| r.transaction_hash)
                .collect::<Vec<H256>>();
            it_info_print!("Block receipts hashes ?", block_receipt_hashes);

            // if the received tx hash is not the one we are interested in, skip
            if !block_receipt_hashes.contains(&tx_receipt.transaction_hash) {
                continue;
            }
            tx_hashes_set.insert(canon_state_notification.engine_index);
            let syncing_instances = suite.global_context.syncing_instances as usize;
            if tx_hashes_set.len() == test_fed_members.len() - syncing_instances {
                break;
            }
        }
    }
    it_info_print!("Block receipts verified");
    // TODO add utxos to one peer and not others
    // build another block and verify that the node's utxos are synced
    let mut target_client = btc_server_clients.get(0).cloned().unwrap();

    // Reset all UTXOs for selected member
    target_client.reset_all_utxos(client::ResetAllUtxosRequest { utxos: vec![] }).await.unwrap();

    // Create a another eoa which should kick off utxo sync
    let botanix_eth_client =
        targeted_fed_member.botanix_eth_client.clone().expect("Botanix Client must be initialized");
    // send eoa messages to the node at selected index
    it_info_print!("Sending eoa transaction...");
    let tx_receipt = botanix_eth_client
        .send_eoa(ethers::core::types::Address::random(), SEND_AMOUNT)
        .await
        .unwrap()
        .unwrap();
    it_info_print!("Eoa tx receipt hash: {:?}", tx_receipt.transaction_hash);

    // wait for canonical chain updates reported by the node, then send new tx
    // wait for fed members to sync on the block
    tokio::time::sleep(Duration::from_secs(10)).await;
    let mut hash_set = HashSet::new();
    for (_index, client) in poa_eth_clients.iter().enumerate() {
        let block_hash = client.get_latest_block_hash().await.unwrap();
        hash_set.insert(block_hash.clone());
    }
    // Everyone should be one the same block
    assert_eq!(hash_set.len(), 1);

    // let header = eth_clients[0].get_latest_block_by_hash(hash);
    let mut hash_set = HashSet::new();
    for client in btc_server_clients.iter_mut() {
        let wallet_state = client.get_wallet_state(client::Empty {}).await.unwrap().into_inner();
        hash_set.insert(wallet_state.wallet_state_commitment);
    }
    // This asserts that the node that was reset is now in sync with the other nodes
    assert_eq!(hash_set.len(), 1);

    // Lets compare the merkel root of the utxo set from the btc server with the latest block header
    let latest_extra_data = poa_eth_clients[0].get_latest_block().await.unwrap().extra_data;

    let _latest_edh =
        ExtraDataHeader::deserialize(&mut latest_extra_data.to_vec().as_slice()).unwrap();
    Ok(())
}
