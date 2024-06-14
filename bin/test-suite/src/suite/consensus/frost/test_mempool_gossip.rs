use reth::consensus_common::utils::{current_inturn_index, unix_timestamp};

use crate::{
    it_info_print,
    suite::consensus::{
        common::{events::SEND_AMOUNT, poa_node::Notifications},
        ConsensusIntegrationTestSuite,
    },
};

/// test that nodes will propogate txs using mempool gossip
pub async fn test_mempool_gossip(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), super::error::Error> {
    let test_fed_members = suite.local_context.poa_nodes.as_ref().unwrap();
    let mut rx = suite.local_context.poa_notification.as_ref().expect("poa notifs").subscribe();
    // get total authorities number
    let total_authorities = test_fed_members.len();

    // Pick an authority member that is not inturn
    // Send the eoa to them and they should propogate it to the inturn member
    let inturn_member_index = (current_inturn_index(total_authorities as u64, unix_timestamp())
        + 1)
        % total_authorities as u64;
    it_info_print!("Inturn member index", inturn_member_index);

    // assign targeted fed memeber
    let targeted_fed_member = test_fed_members.get(&(inturn_member_index as u16)).cloned().unwrap();

    // create eth client
    let botanix_eth_client = targeted_fed_member.create_botanix_eth_client().await;

    // send eoa messages to the node at selected index
    it_info_print!("Sending eoa transaction...");
    let eoa_receiver = ethers::core::types::Address::random();
    it_info_print!("Eoa receiver: {:?}", eoa_receiver.to_string());
    let last_tx_hash =
        botanix_eth_client.send_eoa(eoa_receiver, SEND_AMOUNT).await.unwrap().unwrap();
    it_info_print!("Eoa tx: {:?}", last_tx_hash);

    // wait for canonical chain updates reported by the node, then send new tx
    while let Ok(notification) = rx.recv().await {
        if let Notifications::CanonState(canon_state_notification) = notification {
            it_info_print!(
                "Received payload from engine index",
                canon_state_notification.engine_index
            );
            // block producer and targeted fed member should NOT be the same
            // Look at how inturn_member_index is calculated
            assert_ne!(canon_state_notification.engine_index, inturn_member_index as u16);

            // block verfication
            let block_receipts = canon_state_notification.notification.block_receipts();
            it_info_print!("Block receipts ?", block_receipts);
            assert_eq!(block_receipts.len(), 1);
            let block_payload = block_receipts.first().cloned().unwrap();
            assert!(!block_payload.1);
            assert_eq!(block_payload.0.tx_receipts.len(), 1);
            assert!(block_payload.0.block.number > 0);

            return Ok(());
        }
    }

    Ok(())
}
