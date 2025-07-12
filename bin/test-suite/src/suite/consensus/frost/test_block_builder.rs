use botanix_authority_edh::extra_data_header::ExtraDataHeader;
use ethers::types::H256;
use std::{collections::HashSet, str::FromStr};

use crate::{
    it_info_print,
    suite::consensus::{
        common::{events::SEND_AMOUNT, poa_node::Notifications},
        ConsensusIntegrationTestSuite,
    },
};

#[allow(clippy::too_many_lines)]
pub async fn block_builder(
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

    // take the first member as the target member
    let target_member_index = 0;

    // assign targeted fed member
    let targeted_fed_member = test_fed_members.get(&(target_member_index as u16)).cloned().unwrap();

    // create a minting contract instance
    let botanix_eth_client =
        targeted_fed_member.botanix_eth_client.clone().expect("Botanix Client must be initialized");

    let addr = reth_primitives::Address::from_str(&suite.global_context.botanix_fee_recipient)
        .expect("valid eth address");
    let botanix_block_reward_address_balance_before =
        botanix_eth_client.get_botanix_balance(addr).await.unwrap();
    it_info_print!(
        "Botanix block fee recipient balance before",
        botanix_block_reward_address_balance_before
    );

    let addr = reth_primitives::Address::from_str(&suite.global_context.lst_fee_receiver)
        .expect("valid eth address");
    let lst_fee_receiver_address_balance_before =
        botanix_eth_client.get_botanix_balance(addr).await.unwrap();
    it_info_print!("LST FeeReceiver balance before", lst_fee_receiver_address_balance_before);

    // create a hashmap to store tx hashes
    let mut tx_hashes_set = HashSet::new();

    // send eoa messages to the node at selected index
    it_info_print!("Sending eoa transaction...");
    let eoa_receiver = ethers::core::types::Address::random();
    it_info_print!("Eoa receiver: {:?}", eoa_receiver.to_string());
    let tx_receipt = botanix_eth_client.send_eoa(eoa_receiver, SEND_AMOUNT).await.unwrap().unwrap();
    it_info_print!("Eoa tx receipt hash: {:?}", tx_receipt.transaction_hash);
    tx_hashes_set.insert(tx_receipt.transaction_hash);

    // wait for canonical chain updates reported by the node, then send new tx
    let mut tx_hashes_set: HashSet<u16> = HashSet::new();
    let mut block_fee_recipient_address: Option<reth_primitives::Address> = None;
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

            if block_fee_recipient_address.is_none() {
                let extra_data = canon_state_notification.block.extra_data.0.to_vec();
                let edh = ExtraDataHeader::deserialize(&mut extra_data.as_slice()).unwrap();
                block_fee_recipient_address = Some(edh.block_fee_recipient_address);
            }

            // if the received tx hash is not the one we are interested in, skip
            if !block_receipt_hashes.contains(&tx_receipt.transaction_hash) {
                continue;
            }
            tx_hashes_set.insert(canon_state_notification.engine_index);
            // We need to remove the syncing node from the count
            if tx_hashes_set.len() == test_fed_members.len() - 1 {
                break;
            }
        }
    }

    it_info_print!("All members accepted the block");

    // Check that all members accepted the block
    for (_index, _fed_member_config) in test_fed_members.iter() {
        // verify 80/20 block reward split is correct
        let addr = reth_primitives::Address::from_str(&suite.global_context.botanix_fee_recipient)
            .expect("valid eth address");
        let botanix_block_reward_address_balance_after =
            botanix_eth_client.get_botanix_balance(addr).await.unwrap();
        it_info_print!(
            "Botanix block reward address balance after",
            botanix_block_reward_address_balance_after
        );

        let addr = reth_primitives::Address::from_str(&suite.global_context.lst_fee_receiver)
            .expect("valid eth address");
        let lst_fee_receiver_address_balance_after =
            botanix_eth_client.get_botanix_balance(addr).await.unwrap();
        it_info_print!("LST FeeReceiver  balance after", lst_fee_receiver_address_balance_after);

        let block_fee_recipient_address = block_fee_recipient_address.unwrap();
        // Since the fed member has never produced a block until now,
        // the entire balance should be the block fee reward.
        let fed_member_balance =
            botanix_eth_client.get_botanix_balance(block_fee_recipient_address).await.unwrap();
        it_info_print!("Fed member balance", fed_member_balance);

        let botanix_block_reward = botanix_block_reward_address_balance_after -
            botanix_block_reward_address_balance_before;

        let lst_fee_receiver_block_reward =
            lst_fee_receiver_address_balance_after - lst_fee_receiver_address_balance_before;

        let total_block_reward =
            fed_member_balance + botanix_block_reward + lst_fee_receiver_block_reward;

        assert_eq!(lst_fee_receiver_block_reward, (total_block_reward * 50) / 100); // 50%
        assert_eq!(botanix_block_reward, (total_block_reward * 40) / 100); // 40%
        assert_eq!(
            fed_member_balance,
            (total_block_reward - lst_fee_receiver_block_reward - botanix_block_reward)
        ); // 10%
    }

    Ok(())
}
