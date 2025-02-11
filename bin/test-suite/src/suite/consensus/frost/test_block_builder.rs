use ethers::types::H256;
use reth_primitives::extra_data_header::ExtraDataHeader;

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
    let mut block_producer_address: Option<reth_primitives::Address> = None;
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

            if block_producer_address.is_none() {
                let extra_data = canon_state_notification.block.extra_data.0.to_vec();
                let edh = ExtraDataHeader::deserialize(&mut extra_data.as_slice()).unwrap();
                block_producer_address = Some(edh.block_producer_address);
            }

            // if the received tx hash is not the one we are interested in, skip
            if !block_receipt_hashes.contains(&tx_receipt.transaction_hash) {
                continue;
            }
            tx_hashes_set.insert(canon_state_notification.engine_index);
            if tx_hashes_set.len() != test_fed_members.len() {
                return Ok(());
            }
        }
    }

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

        let block_producer_address = block_producer_address.unwrap();
        let fed_member_balance =
            botanix_eth_client.get_botanix_balance(block_producer_address).await.unwrap();
        it_info_print!("Fed member balance", fed_member_balance);

        let botanix_block_reward = botanix_block_reward_address_balance_after -
            botanix_block_reward_address_balance_before;

        let total_block_reward = fed_member_balance + botanix_block_reward;

        assert_eq!(fed_member_balance, (total_block_reward * 4) / 5); // 80%
        assert_eq!(botanix_block_reward, total_block_reward / 5); // 20%
    }

    Ok(())
}
