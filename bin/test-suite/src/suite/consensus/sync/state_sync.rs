use bitcoincore_rpc::RpcApi;
use reth_provider::SnapshotReader;

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
    utils::generate_blocks,
};

#[allow(clippy::too_many_lines)]
pub async fn state_sync(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), super::error::Error> {
    it_info_print!("Running state sync test...");
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

    // take the first member as the target member
    let target_member_index = 0;

    // assign targeted fed member
    let targeted_fed_member = test_fed_members.get(&(target_member_index as u16)).cloned().unwrap();
    it_info_print!("Max Snapshot Chunk Size Bytes", targeted_fed_member.max_snapshot_size_bytes);

    // create a minting contract instance
    let botanix_eth_client =
        targeted_fed_member.botanix_eth_client.clone().expect("Botanix Client must be initialized");

    // create a hashmap to store tx hashes
    let mut tx_hashes_set = HashSet::new();

    // send eoa messages to random addresses
    for _ in 0..5 {
        it_info_print!("Sending eoa transaction...");
        let eoa_receiver = ethers::core::types::Address::random();
        it_info_print!("Eoa receiver: {:?}", eoa_receiver.to_string());
        let tx_receipt =
            botanix_eth_client.send_eoa(eoa_receiver, SEND_AMOUNT).await.unwrap().unwrap();
        it_info_print!("Eoa tx receipt hash: {:?}", tx_receipt.transaction_hash);
        tokio::time::sleep(Duration::from_millis(200)).await;
        tx_hashes_set.insert(tx_receipt.transaction_hash);
    }

    // wait for canonical chain updates reported by the node, then send new tx
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
            let db_provider = suite
                .local_context
                .get_dbs()
                .get(canon_state_notification.engine_index as usize)
                .cloned()
                .unwrap();
            let snapshots = db_provider.get_snapshots().unwrap_or_default();
            // NOTE: at these point we should have 2 snapshots in the db, the first one being
            // finalized and the second one being in progress
            if snapshots.len() == 2 {
                let first_snapshot_block_id = snapshots.first().unwrap().height();
                let snapshot_id = db_provider
                    .get_snapshot_id_by_block_id(first_snapshot_block_id)
                    .unwrap()
                    .unwrap();
                let data_parser =
                    DataParser::default().with_serialization_type(SerializationType::Postcard);
                let snapshot_chunks_data =
                    db_provider.assemble_snapshot_chunks_data(snapshot_id).unwrap();
                for (block, block_chunks) in snapshot_chunks_data {
                    let sealed_block =
                        data_parser.decode::<SealedBlockWithSenders>(&block_chunks).await;
                    assert!(sealed_block.is_ok());
                    let sealed_block = sealed_block.expect("must be a block");
                    assert!(sealed_block.block.header().number == block);
                }
                break;
            }
        }
    }

    Ok(())
}
