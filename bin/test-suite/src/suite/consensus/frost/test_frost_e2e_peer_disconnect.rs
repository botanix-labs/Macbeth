use std::time::Duration;

use reth::consensus_common::utils::{current_inturn_index, unix_timestamp};

use crate::{
    it_info_print,
    suite::consensus::{
        common::{
            events::SEND_AMOUNT,
            poa_node::{Notifications, TestSignal},
        },
        ConsensusIntegrationTestSuite,
    },
};

/// test that nodes will propogate txs using mempool gossip
pub async fn frost_e2e_peer_disconnect(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), super::error::Error> {
    let test_fed_members = suite.local_context.poa_nodes.as_ref().unwrap();
    let mut rx = suite.local_context.poa_notification.as_ref().expect("poa notifs").subscribe();

    // assign targeted fed memeber
    let targeted_fed_member = test_fed_members.get(&(0u16)).cloned().unwrap();

    // now disconnect the peers of fed member
    targeted_fed_member.send_test_signal(TestSignal::DisconnectAll());

    // wait for the disconnected peer to be seen
    tokio::time::sleep(Duration::from_secs(30)).await;

    // now reconnect the peers of fed member
    targeted_fed_member.send_test_signal(TestSignal::ReconnectAll());

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
