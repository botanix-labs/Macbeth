use bitcoincore_rpc::RpcApi;
use reth::{
    consensus_common::utils::{current_inturn_index, unix_timestamp},
    primitives::public_key_to_address,
};
use reth_chainspec::BOTANIX_TESTNET;
use reth_primitives::{header_ext::HeaderExt, U256};

use std::{collections::HashSet, str::FromStr, time::Duration};

use crate::{
    it_info_print,
    suite::consensus::{
        common::{
            botanix_client::BotanixEthClient,
            events::{BITCOIND_WALLET_NAME, SEND_AMOUNT},
            poa_node::Notifications,
        },
        ConsensusIntegrationTestSuite,
    },
    utils::generate_blocks,
};

#[allow(clippy::too_many_lines)]
pub async fn block_builder(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), super::error::Error> {
    it_info_print!("Running block builder test...");
    let leader_selection_window =
        BOTANIX_TESTNET.leader_selection_window.clone().expect("block times");
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
    let _address =
        bitcoind_rpc.get_new_address(None, None).expect("get new address").assume_checked();
    // generate > 100 blocks so coinbase utxos can be spent from the wallet
    generate_blocks(&bitcoind_rpc, 101).await;
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
    let inturn_member_index =
        current_inturn_index(total_authorities as u64, unix_timestamp(), leader_selection_window);

    // assign targeted fed member
    let targeted_fed_member = test_fed_members.get(&(inturn_member_index as u16)).cloned().unwrap();

    // create a minting contract instance
    let botanix_eth_client =
        targeted_fed_member.botanix_eth_client.clone().expect("Botanix Client must be initialized");

    let botanix_block_reward_address_balance_before = botanix_eth_client
        .get_botanix_balance(&suite.global_context.botanix_fee_recipient)
        .await
        .unwrap();
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
    let last_tx_hash =
        botanix_eth_client.send_eoa(eoa_receiver, SEND_AMOUNT).await.unwrap().unwrap();
    it_info_print!("Eoa tx: {:?}", last_tx_hash);
    tx_hashes_set.insert(last_tx_hash.transaction_hash);

    // retrieve the current aggregate public key
    let aggregate_public_key_str =
        suite.local_context.btc_server_clients.clone().expect("btc server clients")[0]
            .get_public_key(client::Empty {})
            .await
            .unwrap()
            .into_inner()
            .publickey;

    let aggregate_public_key = secp256k1::PublicKey::from_str(&aggregate_public_key_str).unwrap();
    // Oof this is nasty, lets just put authorities in a vec in local context
    let authority_signers = &suite
        .local_context
        .poa_nodes
        .as_ref()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .authorities
        .clone();

    // wait for canonical chain updates reported by the node, then send new tx
    while let Ok(notification) = rx.recv().await {
        if let Notifications::CanonState(canon_state_notification) = notification {
            it_info_print!(
                "Received payload from engine index",
                canon_state_notification.engine_index
            );

            // wait for fed members to sync on the block
            tokio::time::sleep(Duration::from_secs(9)).await;

            // Check that all members accepted the block
            let mut botanix_clients: Vec<BotanixEthClient> = vec![];
            for (index, fed_member_config) in test_fed_members.iter() {
                let botanix_eth_client = fed_member_config
                    .botanix_eth_client
                    .clone()
                    .expect("Botanix Client must be initialized");
                botanix_clients.push(botanix_eth_client);
                it_info_print!("Botanix client created for poa member {}", index);
            }
            let latest_block_hash = canon_state_notification.notification.tip().hash();
            for (index, client) in botanix_clients.iter().enumerate() {
                let block_hash = client.get_latest_block_hash().await.unwrap();
                it_info_print!(
                    "Botanix client",
                    format!("index={index}: block hash - {block_hash}")
                );

                assert_eq!(block_hash.as_bytes(), latest_block_hash.as_slice());
            }

            let header = canon_state_notification.notification.tip().header();
            let edh = header.deserialize_extra_data_header().unwrap();
            assert_eq!(edh.aggregated_public_key, aggregate_public_key);

            let block_receipts = canon_state_notification.notification.block_receipts();
            it_info_print!("Block receipts ?", block_receipts);
            assert_eq!(block_receipts.len(), 1);
            let block_payload = block_receipts.first().cloned().unwrap();
            assert!(!block_payload.1);
            assert_eq!(block_payload.0.tx_receipts.len(), 1);
            assert!(block_payload.0.block.number > 0);

            // get fed member and botanix block reward address balances
            it_info_print!("Botanix block fee recipient");

            info!("authority signer length: {}", authority_signers.len());
            let fed_member_pub_key = suite
                .local_context
                .authorities
                .get((canon_state_notification.engine_index) as usize)
                .unwrap();
            let fed_member_ethereum_address =
                public_key_to_address(*fed_member_pub_key).to_checksum(Some(3636));

            let fed_member_balance = botanix_eth_client
                .get_botanix_balance(fed_member_ethereum_address.as_str())
                .await
                .unwrap();
            it_info_print!("Fed member balance", fed_member_balance);

            // verify 80/20 block reward split is correct
            if fed_member_balance > U256::ZERO.into() {
                let botanix_block_reward_address_balance_after = botanix_eth_client
                    .get_botanix_balance(&suite.global_context.botanix_fee_recipient)
                    .await
                    .unwrap();
                it_info_print!(
                    "Botanix block reward address balance after",
                    botanix_block_reward_address_balance_after
                );

                let botanix_block_reward = botanix_block_reward_address_balance_after -
                    botanix_block_reward_address_balance_before;

                let total_block_reward = fed_member_balance + botanix_block_reward;

                assert_eq!(fed_member_balance, (total_block_reward * 4) / 5); // 80%
                assert_eq!(botanix_block_reward, total_block_reward / 5); // 20%

                return Ok(());
            }
        }
    }

    Ok(())
}
