use ethers::types::H256;
use std::collections::HashSet;

use crate::{
    it_info_print,
    suite::consensus::{
        common::{events::SEND_AMOUNT, poa_node::Notifications},
        ConsensusIntegrationTestSuite,
    },
};

/// test that nodes will propagate txs using mempool gossip
pub async fn test_mempool_gossip(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), super::error::Error> {
    let test_fed_members = suite.local_context.poa_nodes.as_ref().unwrap();
    let mut rx = suite.local_context.poa_notification.as_ref().expect("poa notifs").subscribe();

    let selected_member_index = 0;
    it_info_print!("Selected member index", selected_member_index);

    // assign targeted fed member
    let targeted_fed_member =
        test_fed_members.get(&(selected_member_index as u16)).cloned().unwrap();

    // create eth client
    let botanix_eth_client =
        targeted_fed_member.botanix_eth_client.clone().expect("Botanix Client must be initialized");

    // send eoa messages to the node at selected index
    it_info_print!("Sending eoa transaction...");
    let eoa_receiver = ethers::core::types::Address::random();
    it_info_print!("Eoa receiver: {:?}", eoa_receiver.to_string());
    let tx_receipt = botanix_eth_client.send_eoa(eoa_receiver, SEND_AMOUNT).await.unwrap().unwrap();
    it_info_print!("Eoa tx receipt hash: {:?}", tx_receipt.transaction_hash);

    let mut tx_hashes_set: HashSet<u16> = HashSet::new();
    // wait for canonical chain updates reported by the node, then send new tx
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

            // block verification
            let block_receipt_hashes = canon_state_notification
                .tx_receipts
                .iter()
                .map(|r| r.transaction_hash)
                .collect::<Vec<H256>>();
            it_info_print!("Block receipts hashes ?", block_receipt_hashes);

            if block_receipt_hashes.contains(&tx_receipt.transaction_hash) {
                tx_hashes_set.insert(canon_state_notification.engine_index);
            }
            if tx_hashes_set.len() == test_fed_members.len() {
                return Ok(());
            }
        }
    }

    Ok(())
}
