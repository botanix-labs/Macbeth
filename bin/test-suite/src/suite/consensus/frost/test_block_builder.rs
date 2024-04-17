use reth::core::cli::runner::CliRunner;
use reth::primitives::{constants::BOTANIX_FEES_RECIPIENT, public_key_to_address};
use reth_botanix_lib::extra_data_header::ExtraDataHeader;
use std::{collections::HashSet, time::Duration};

use crate::{
    it_info_print,
    suite::consensus::{
        frost::{
            poa_node::{create_poa_federation_members, is_inturn, Notifications},
            test_frost_e2e::await_dkg,
        },
        ConsensusIntegrationTestSuite,
    },
};

const SEND_AMOUNT: u64 = 1; // = 1 Botanix BTC
const SELECTED_FED_MEMBER_INDEX: usize = 0;

pub async fn block_builder(
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
            let fed_member_command = fed_member_config.build_command();
            let runner = CliRunner::default();
            runner.run_command_until_exit(|ctx| fed_member_command.execute(ctx)).unwrap();
        });
        // wait for one second inbetween members start
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // wait for the dkg to finish for each of them
    await_dkg(&mut test_fed_members, &mut rx).await;

    // assign targeted fed memeber
    let targeted_fed_member =
        test_fed_members.get(&(SELECTED_FED_MEMBER_INDEX as u16)).cloned().unwrap();

    // create a minting contract instance
    let botanix_eth_client = targeted_fed_member.create_botanix_eth_client().await;

    // get fed member and botanix address balances
    let edh = targeted_fed_member.edh.unwrap();
    let targeted_fed_member_pub_key = *edh.authority_signers.unwrap().first().unwrap();
    let targeted_fed_member_ethereum_address =
        public_key_to_address(targeted_fed_member_pub_key).to_checksum(Some(3636));

    let target_fed_member_balance_before = botanix_eth_client
        .get_botanix_balance(targeted_fed_member_ethereum_address.as_str())
        .await
        .unwrap();
    it_info_print!("Targeted fed member balance before: {}", target_fed_member_balance_before);

    let botanix_block_reward_address_balance_before =
        botanix_eth_client.get_botanix_balance(BOTANIX_FEES_RECIPIENT).await.unwrap();
    it_info_print!(
        "Botanix block fee recipient balance before: {}",
        botanix_block_reward_address_balance_before
    );

    // create a hashmap to store tx hashes
    let mut tx_hashes_set = HashSet::new();

    // wait until the preselected fed member becomes inturn
    'inner: loop {
        let is_test_fed_member_inturn =
            is_inturn(total_authorities as u64, targeted_fed_member.index.into());
        it_info_print!("Is in turn?", is_test_fed_member_inturn);
        if is_test_fed_member_inturn {
            break 'inner;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        continue;
    }
    it_info_print!("Federation memeber with index = {} is not inturn", targeted_fed_member.index);

    // send eoa messages to the node at selected index
    it_info_print!("Sending eoa transaction...");
    let eoa_receiver = ethers::core::types::Address::random();
    it_info_print!("Eoa receiver: {:?}", eoa_receiver.to_string());
    let last_tx_hash =
        botanix_eth_client.send_eoa(eoa_receiver, SEND_AMOUNT).await.unwrap().unwrap();
    it_info_print!("Eoa tx: {:?}", last_tx_hash);
    tx_hashes_set.insert(last_tx_hash.transaction_hash);

    // wait for canonical chain updates reported by the node, then send new tx
    while let Some(notification) = rx.recv().await {
        match notification {
            Notifications::CanonState(canon_state_notification) => {
                it_info_print!(
                    "Received payload from engine index",
                    canon_state_notification.engine_index
                );
                assert_eq!(canon_state_notification.engine_index, SELECTED_FED_MEMBER_INDEX as u16);

                // block verfication
                if canon_state_notification.engine_index == targeted_fed_member.index {
                    let block_receipts = canon_state_notification.notification.block_receipts();
                    it_info_print!("Block receipts ?", block_receipts);
                    assert_eq!(block_receipts.len(), 1);
                    let block_payload = block_receipts.first().cloned().unwrap();
                    assert!(!block_payload.1);
                    assert_eq!(block_payload.0.tx_receipts.len(), 1);
                    assert!(block_payload.0.block.number > 0);

                    // get fed member and botanix block reward address balances
                    let target_fed_member_balance_after = botanix_eth_client
                        .get_botanix_balance(targeted_fed_member_ethereum_address.as_str())
                        .await
                        .unwrap();
                    it_info_print!(
                        "Targeted fed member balance after: {}",
                        target_fed_member_balance_after
                    );

                    it_info_print!("Botanix block fee recipient: {}", BOTANIX_FEES_RECIPIENT);

                    let botanix_block_reward_address_balance_before_after = botanix_eth_client
                        .get_botanix_balance(BOTANIX_FEES_RECIPIENT)
                        .await
                        .unwrap();
                    it_info_print!(
                        "Botanix block reward address balance after: {}",
                        botanix_block_reward_address_balance_before_after
                    );

                    // verify 80/20 block reward split is correct
                    let target_fed_member_reward =
                        target_fed_member_balance_after - target_fed_member_balance_before;
                    let botanix_block_reward = botanix_block_reward_address_balance_before_after
                        - botanix_block_reward_address_balance_before;

                    let total_block_reward = target_fed_member_reward + botanix_block_reward;

                    assert_eq!(target_fed_member_reward, (total_block_reward * 4) / 5); // 80%
                    assert_eq!(botanix_block_reward, total_block_reward / 5); // 20%

                    return Ok(());
                }
            }
            _ => {}
        }
    }

    Ok(())
}
