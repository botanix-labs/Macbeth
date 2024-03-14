use reth::core::cli::runner::CliRunner;
use std::{collections::HashSet, time::Duration};

use crate::suite::consensus::{
    poa::{
        payload_sender::TestPayloadSender,
        poa_node::{create_poa_federation_members, is_inturn},
    },
    ConsensusIntegrationTestSuite,
};

const RECEIVER_ADDRESS: &'static str = "0x613580C865985dA78613Ea7EBCF7a3b8C5445F93";
const SENDER_SECRET_KEY: &'static str =
    "52947524bbc14bd90cc86c32b9b7564da2f7f8de343825fed68cd04da4925d29";
const SEND_AMOUNT: u64 = 1; // = 1 Botanix
const INTEGRATION_TEST_ROUNDS: u8 = 3;
const SELECTED_FED_MEMBER_INDEX: usize = 0;

pub async fn poa_eoa(suite: &ConsensusIntegrationTestSuite) -> Result<(), super::error::Error> {
    // generate test fed members poa nodes
    let (test_fed_members, mut rx) =
        create_poa_federation_members(&suite.config, suite.local_context.btc_servers.as_ref());

    // assign targeted fed memeber
    let targeted_fed_member = test_fed_members.get(&SELECTED_FED_MEMBER_INDEX).cloned().unwrap();

    // get total authorities number
    let total_authorities = test_fed_members.len();

    // run all poa nodes in the background
    for (_index, fed_member_config) in test_fed_members.into_iter() {
        let _ = std::thread::spawn(move || {
            let fed_member_command = fed_member_config.build_command();
            let runner = CliRunner::default();
            runner.run_command_until_exit(|ctx| fed_member_command.execute(ctx)).unwrap();
        });
        // wait for one second inbetween members start
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // create payload client
    let payload_client =
        TestPayloadSender::new(targeted_fed_member.rpc_port, SENDER_SECRET_KEY).await;

    // create a hashmap to store tx hashes
    let mut tx_hashes_set = HashSet::new();

    // send eoa messages to the node at selected index
    println!("======>  Sending eoa transaction...");
    let mut last_tx_hash = payload_client.send(RECEIVER_ADDRESS, SEND_AMOUNT).await.unwrap();
    tx_hashes_set.insert(last_tx_hash.to_fixed_bytes());

    // wait for canonical chain updates reported by the node, then send new tx
    let test_rounds = 0;
    while let Some(x) = rx.recv().await {
        println!("======> Received payload from engine index {:?}", x.engine_index);
        assert_eq!(x.engine_index, SELECTED_FED_MEMBER_INDEX as u16);
        if test_rounds == INTEGRATION_TEST_ROUNDS {
            break;
        }

        // after first successful tx, send invalid tx with too low nonce
        if test_rounds == 1 {
            println!("======>  Sending eoa transaction with too low nonce...");
            payload_client.send_invalid(RECEIVER_ADDRESS).await;
        }

        // block verfication
        let block_receipts = x.notification.block_receipts();
        println!("Block receipts? {:?}", block_receipts);
        assert_eq!(block_receipts.len(), 1);
        let block_payload = block_receipts.first().cloned().unwrap();
        assert!(!block_payload.1);
        assert_eq!(block_payload.0.tx_receipts.len(), 1);
        assert!(block_payload.0.block.number > 0);

        // wait until current turn changes
        let current_turn = is_inturn(total_authorities as u64, targeted_fed_member.index.into());
        'inner: loop {
            let is_test_fed_member_inturn =
                is_inturn(total_authorities as u64, targeted_fed_member.index.into());
            println!("Is in turn? {}", is_test_fed_member_inturn);
            if is_test_fed_member_inturn != current_turn {
                break 'inner;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        println!("======>  Sending eoa transaction...");
        last_tx_hash = payload_client.send(RECEIVER_ADDRESS, SEND_AMOUNT).await.unwrap();
        tx_hashes_set.insert(last_tx_hash.to_fixed_bytes());
    }

    Ok(())
}
