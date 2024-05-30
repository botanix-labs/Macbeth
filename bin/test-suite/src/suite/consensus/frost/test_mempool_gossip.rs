
use std::time::Duration;

use reth::CliRunner;

use crate::{
    it_info_print,
    suite::consensus::{
        common::{
            events::{await_dkg, SEND_AMOUNT},
            poa_node::{create_poa_federation_members, current_inturn_index, Notifications},
        },
        ConsensusIntegrationTestSuite,
    },
};

/// test that nodes will propogate txs using mempool gossip
pub async fn mempool_gossip(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), super::error::Error> {
    // generate test fed members poa nodes
    let (mut test_fed_members, mut rx) = create_poa_federation_members(
        suite.global_context.clone(),
        suite.local_context.btc_servers.as_ref(),
    )
    .await;

    // get total authorities number
    let total_authorities = test_fed_members.len();

    // run all poa nodes in the background
    for (_index, fed_member_config) in test_fed_members.iter() {
        let fed_member_config = fed_member_config.clone();
        let _ = std::thread::spawn(move || {
            let (fed_member_command, _chain_spec) = fed_member_config.build_command();
            let runner = CliRunner::default();
            runner.run_command_until_exit(|ctx| fed_member_command.execute(ctx)).unwrap();
        });
        // wait for one second inbetween members start
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // wait for the dkg to finish for each of them
    await_dkg(&mut test_fed_members, &mut rx).await;

    // Pick an authority member that is not inturn
    // Send the eoa to them and they should propogate it to the inturn member
    let inturn_member_index =
        (current_inturn_index(total_authorities as u64) + 1) % total_authorities as u64;
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
    while let Some(notification) = rx.recv().await {
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
