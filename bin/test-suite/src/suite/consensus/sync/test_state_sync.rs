use comet_bft_rpc::{Client, CometBftRpcFactory};
use reth_data_parser::{DataParser, SerializationType};
use reth_primitives::BlockWithSenders;
use reth_provider::{BlockNumReader, SnapshotReader};

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use crate::{
    it_info_print,
    suite::consensus::{
        common::{
            comet_node::update_config_toml_with_trusted_height_and_hash, events::SEND_AMOUNT,
            poa_node::Notifications,
        },
        ConsensusIntegrationTestSuite,
    },
};

const MAX_RETRIES: u8 = 5;
const REQUIRED_SNAPSHOTS: usize = 1;

#[allow(clippy::too_many_lines)]
pub async fn test_state_sync(
    suite: &mut ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), anyhow::Error> {
    it_info_print!("Running state sync test...");
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

    // create a hashmap to store tx hashes
    let mut tx_hashes_set = HashSet::new();

    // send eoa messages to random addresses
    // NOTE: this should be enough to trigger the creation of 2 snapshots considering the max
    // snapshot size in test env.
    for _ in 0..5 {
        it_info_print!("Sending eoa transaction...");
        let eoa_receiver = ethers::core::types::Address::random();
        it_info_print!("Eoa receiver:", eoa_receiver.to_string());
        let tx_receipt =
            botanix_eth_client.send_eoa(eoa_receiver, SEND_AMOUNT).await.unwrap().unwrap();
        it_info_print!("Eoa tx receipt hash ", tx_receipt.transaction_hash);
        tokio::time::sleep(Duration::from_millis(200)).await;
        tx_hashes_set.insert(tx_receipt.transaction_hash);
    }

    while let Ok(notification) = rx.recv().await {
        if let Notifications::CanonState(canon_state_notification) = notification {
            it_info_print!(
                "Received payload from engine index",
                canon_state_notification.engine_index
            );
            it_info_print!(
                "Received block number from engine = ",
                canon_state_notification.block.number.map(|n| n.as_u64())
            );
            let db_provider = suite
                .local_context
                .get_dbs()
                .get(canon_state_notification.engine_index as usize)
                .cloned()
                .unwrap();
            let snapshots = db_provider.get_snapshots().unwrap_or_default();
            if snapshots.len() >= REQUIRED_SNAPSHOTS {
                let first_snapshot_block_id = snapshots.first().unwrap().height();
                let snapshot_id = db_provider
                    .get_snapshot_id_by_block_id(first_snapshot_block_id)
                    .unwrap()
                    .unwrap();
                let data_parser =
                    DataParser::default().with_serialization_type(SerializationType::Postcard);
                let snapshot = db_provider.get_snapshot_by_id(snapshot_id).unwrap().unwrap();
                let chunk_ids = snapshot.chunk_ids().to_vec();
                let mut snapshot_chunks_data: Vec<(u64, Vec<Vec<u8>>)> = Vec::new();
                for id in chunk_ids {
                    let chunk = db_provider.get_chunk_by_id(id).unwrap().unwrap();
                    snapshot_chunks_data.push((
                        chunk.get_ending_block_number(),
                        chunk.chunk_data().iter().map(|b| b.as_ref().to_vec()).collect(),
                    ));
                }
                for (ending_block, chunk_data) in snapshot_chunks_data {
                    for chunk in chunk_data {
                        let mut sealed_blocks_saved: Vec<BlockWithSenders> = Vec::new();
                        let sealed_blocks = data_parser.decode::<BlockWithSenders>(&chunk).await;
                        match sealed_blocks {
                            Ok(sealed_block) => {
                                it_info_print!(
                                "Decoded sealed blocks from snapshot chunk with ending block number",
                                ending_block
                            );
                                sealed_blocks_saved.push(sealed_block);
                            }
                            Err(e) => {
                                it_info_print!(
                                    "Error decoding sealed blocks from snapshot chunk",
                                    e
                                );
                            }
                        }
                        assert!(sealed_blocks_saved.last().unwrap().header.number == ending_block);
                    }
                }
                break;
            }
        }
    }
    it_info_print!("Starting dynamic sync");

    // get a cometbft rpc client for any non-syncing poa node
    let (trusted_block_height, trusted_block_hash) =
        if let Some(cometbft_rpc_clients) = suite.local_context.cometbft_rpc_clients.as_ref() {
            let cometrpc =
                cometbft_rpc_clients.get(target_member_index).expect("cometbft rpc client").clone();
            let cometbft_http_client = cometrpc.build_and_connect().expect("to be connected");
            let trusted_block_height = 1u32;
            let trusted_block_hash = cometbft_http_client
                .block(trusted_block_height)
                .await
                .expect("to have first block")
                .block
                .header
                .hash();
            it_info_print!("trusted block hash at height 1", trusted_block_hash);
            let latest_block =
                cometbft_http_client.latest_block().await.unwrap().block.header().height.value();
            it_info_print!("latest cometbft block height", latest_block);
            (trusted_block_height, trusted_block_hash)
        } else {
            panic!("No trusted block height and hash");
        };

    let latest_botanix_block = botanix_eth_client.get_latest_block().await.unwrap();
    it_info_print!(
        "latest botanix block height",
        latest_botanix_block.number.unwrap_or_default().as_u64()
    );

    // wait until all poas have at least a snapshots to sync against
    let member_ids = suite
        .local_context
        .poa_nodes
        .clone()
        .unwrap_or_default()
        .keys()
        .cloned()
        .collect::<Vec<u16>>();
    it_info_print!("syncing instances", suite.global_context.syncing_instances);
    // TODO: export a method on local_context to get the length of non-syncing nodes
    let member_ids: Vec<u16> = member_ids
        [..member_ids.len().saturating_sub(suite.global_context.syncing_instances as usize)] // remove the syncing nodes
        .to_vec();
    let mut snapshots_per_fed_member: HashMap<u16, usize> = HashMap::new();
    let expected_sync_height = 'outer: loop {
        for memeber_id in member_ids.clone() {
            let db_provider =
                suite.local_context.get_dbs().get(memeber_id as usize).cloned().unwrap();
            let snapshots = db_provider.get_snapshots().unwrap_or_default();
            snapshots_per_fed_member.insert(memeber_id, snapshots.len());

            let expected_sync_height =
                snapshots.first().as_ref().map(|s| s.height()).unwrap_or_default();

            let insuficient_snapshots = snapshots_per_fed_member
                .iter()
                .any(|(_, snapshots)| *snapshots < REQUIRED_SNAPSHOTS);
            if !insuficient_snapshots {
                break 'outer expected_sync_height;
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    it_info_print!("All nodes have produced a snapshot");
    it_info_print!("Expected sync height", expected_sync_height);

    // start the syncing cometbft node
    if let Some(cometbft_nodes_syncing) = suite.local_context.cometbft_nodes_syncing.as_ref() {
        for (_index, comet_node) in cometbft_nodes_syncing.iter() {
            if let Some(spawned_cometbft_processes) =
                suite.local_context.cometbft_processes.as_mut()
            {
                // overwrite the config with the trusted block hash and height
                update_config_toml_with_trusted_height_and_hash(
                    &comet_node,
                    trusted_block_height as i64,
                    &trusted_block_hash.to_string(),
                )
                .unwrap();
                // spawn the comet process
                spawned_cometbft_processes.push(comet_node.spawn_service().unwrap());
                // await initialization
                comet_node.await_initialization().unwrap();
            }
        }
    }

    // get the syncing node db
    let db_provider_syncing_member =
        suite.local_context.get_dbs().get(member_ids.len()).cloned().unwrap();

    let mut retries = 0;
    loop {
        let last_block_number = db_provider_syncing_member.last_block_number().unwrap();
        it_info_print!("Syncing last block number ", last_block_number);

        if last_block_number >= expected_sync_height {
            return Ok(());
        }

        retries += 1;
        if retries >= MAX_RETRIES {
            return Err(anyhow::anyhow!("Syncing failed after {} retries!", MAX_RETRIES));
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
