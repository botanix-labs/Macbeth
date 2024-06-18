use bitcoincore_rpc::{Auth, RpcApi};
use reth::{
    consensus_common::utils::{current_inturn_index, is_inturn, unix_timestamp},
    primitives::{constants::BOTANIX_FEES_RECIPIENT, public_key_to_address},
};

use std::{collections::HashSet, time::Duration};

use crate::{
    it_info_print,
    suite::consensus::{
        common::{
            events::{BITCOIND_WALLET_NAME, SEND_AMOUNT},
            poa_node::Notifications,
        },
        ConsensusIntegrationTestSuite,
    },
};

pub async fn block_builder(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), super::error::Error> {
    it_info_print!("Running block builder test...");
    let bitcoind_rpc = suite.global_context.bitcoind_rpc();

    // Load up the bitcoin wallet and generate some blocks
    for wallet in bitcoind_rpc.list_wallets().unwrap() {
        it_info_print!("#UNLOADING WALLET?", &wallet);
        let _ = bitcoind_rpc.unload_wallet(Some(&wallet));
    }
    let create_res = bitcoind_rpc.create_wallet(BITCOIND_WALLET_NAME, None, None, None, None);
    if create_res.is_err() {
        // wallet already exists
        // load wallet
        let _ = bitcoind_rpc.load_wallet(BITCOIND_WALLET_NAME);
    }
    let address =
        bitcoind_rpc.get_new_address(None, None).expect("get new address").assume_checked();
    // generate > 100 blocks so coinbase utxos can be spent from the wallet
    bitcoind_rpc.generate_to_address(101, &address).expect("generate to address");
    // sleep and wait for poa nodes to register this block
    tokio::time::sleep(Duration::from_secs(5)).await;

    let test_fed_members = suite
        .local_context
        .poa_nodes
        .as_ref()
        .expect("test federation member configurations")
        .clone();
    let mut rx = suite.local_context.poa_notification.as_ref().expect("poa notifs").subscribe();

    // get total authorities number
    let total_authorities = test_fed_members.len();

    // find out who is in turn
    let inturn_member_index = current_inturn_index(total_authorities as u64, unix_timestamp());

    // assign targeted fed memeber
    let targeted_fed_member = test_fed_members.get(&(inturn_member_index as u16)).cloned().unwrap();

    // create a minting contract instance
    let botanix_eth_client = targeted_fed_member.create_botanix_eth_client().await;

    // get fed member and botanix address balances
    let edh = targeted_fed_member.edh.unwrap();
    let targeted_fed_member_pub_key =
        *edh.authority_signers.unwrap().get(targeted_fed_member.index as usize).unwrap();
    let targeted_fed_member_ethereum_address =
        public_key_to_address(targeted_fed_member_pub_key).to_checksum(Some(3636));

    let target_fed_member_balance_before = botanix_eth_client
        .get_botanix_balance(targeted_fed_member_ethereum_address.as_str())
        .await
        .unwrap();
    it_info_print!("Targeted fed member balance before", target_fed_member_balance_before);

    let botanix_block_reward_address_balance_before =
        botanix_eth_client.get_botanix_balance(BOTANIX_FEES_RECIPIENT).await.unwrap();
    it_info_print!(
        "Botanix block fee recipient balance before",
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
    while let Ok(notification) = rx.recv().await {
        if let Notifications::CanonState(canon_state_notification) = notification {
            it_info_print!(
                "Received payload from engine index",
                canon_state_notification.engine_index
            );

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
                    "Targeted fed member balance after",
                    target_fed_member_balance_after
                );

                it_info_print!("Botanix block fee recipient", BOTANIX_FEES_RECIPIENT);

                let botanix_block_reward_address_balance_before_after =
                    botanix_eth_client.get_botanix_balance(BOTANIX_FEES_RECIPIENT).await.unwrap();
                it_info_print!(
                    "Botanix block reward address balance after",
                    botanix_block_reward_address_balance_before_after
                );

                // verify 80/20 block reward split is correct
                let target_fed_member_reward =
                    target_fed_member_balance_after - target_fed_member_balance_before;
                let botanix_block_reward = botanix_block_reward_address_balance_before_after -
                    botanix_block_reward_address_balance_before;

                let total_block_reward = target_fed_member_reward + botanix_block_reward;

                assert_eq!(target_fed_member_reward, (total_block_reward * 4) / 5); // 80%
                assert_eq!(botanix_block_reward, total_block_reward / 5); // 20%

                return Ok(());
            }
        }
    }

    Ok(())
}
