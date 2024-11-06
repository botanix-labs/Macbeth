use bitcoincore_rpc::RpcApi;
use ethers::types::H256;
use reth::primitives::public_key_to_address;

use std::{collections::HashSet, str::FromStr, time::Duration};

use crate::{
    it_info_print,
    suite::consensus::{
        common::{
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

    // take the first member as the inturn member
    let inturn_member_index = 0;

    // assign targeted fed member
    let targeted_fed_member = test_fed_members.get(&(inturn_member_index as u16)).cloned().unwrap();

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

    // retrieve the current aggregate public key
    let aggregate_public_key_str =
        suite.local_context.btc_server_clients.clone().expect("btc server clients")[0]
            .get_public_key(client::Empty {})
            .await
            .unwrap()
            .into_inner()
            .publickey;

    let _aggregate_public_key = secp256k1::PublicKey::from_str(&aggregate_public_key_str).unwrap();
    // Oof this is nasty, lets just put authorities in a vec in local context
    let _authority_signers = &suite
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
    let mut tx_hashes_set: HashSet<u16> = HashSet::new();
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

            // if the received tx hash is not the one we are interested in, skip
            if !block_receipt_hashes.contains(&tx_receipt.transaction_hash) {
                continue;
            }
            tx_hashes_set.insert(canon_state_notification.engine_index);

            let fed_member_pub_key = suite
                .local_context
                .authorities
                .get((canon_state_notification.engine_index) as usize)
                .unwrap();
            let fed_member_ethereum_address = public_key_to_address(*fed_member_pub_key);

            let fed_member_balance =
                botanix_eth_client.get_botanix_balance(fed_member_ethereum_address).await.unwrap();
            it_info_print!("Fed member balance", fed_member_balance);

            if tx_hashes_set.len() == test_fed_members.len() {
                return Ok(());
            }
        }
    }

    // Check that all members accepted the block
    for (index, fed_member_config) in test_fed_members.iter() {
        let botanix_eth_client = fed_member_config
            .botanix_eth_client
            .clone()
            .expect("Botanix Client must be initialized");
        let fed_member_pub_key = suite.local_context.authorities.get(*index as usize).unwrap();
        let fed_member_ethereum_address = public_key_to_address(*fed_member_pub_key);
        let fed_member_balance =
            botanix_eth_client.get_botanix_balance(fed_member_ethereum_address).await.unwrap();
        it_info_print!("Fed member balance", fed_member_balance);

        // verify 80/20 block reward split is correct
        // TODO: fix this
        // if fed_member_balance > U256::ZERO.into() {
        //     let addr =
        // reth_primitives::Address::from_str(&suite.global_context.botanix_fee_recipient).expect("
        // valid eth address");     let botanix_block_reward_address_balance_after =
        // botanix_eth_client         .get_botanix_balance(addr)
        //         .await
        //         .unwrap();
        //     it_info_print!(
        //         "Botanix block reward address balance after",
        //         botanix_block_reward_address_balance_after
        //     );

        //     let botanix_block_reward = botanix_block_reward_address_balance_after -
        //         botanix_block_reward_address_balance_before;

        //     let total_block_reward = fed_member_balance + botanix_block_reward;

        //     assert_eq!(fed_member_balance, (total_block_reward * 4) / 5); // 80%
        //     assert_eq!(botanix_block_reward, total_block_reward / 5); // 20%

        //     return Ok(());
        // }
    }

    Ok(())
}
