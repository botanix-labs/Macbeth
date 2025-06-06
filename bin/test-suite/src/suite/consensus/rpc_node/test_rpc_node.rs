use anyhow::Context;
use ethers::types::U64;

use crate::{
    it_info_print,
    suite::consensus::{
        common::{
            botanix_client::BotanixEthClient, events::SEND_AMOUNT, PREFUNDED_ACCOUNT_SECRET_KEY,
        },
        ConsensusIntegrationTestSuite,
    },
};

#[allow(clippy::unwrap_used, clippy::cast_possible_truncation)]
pub async fn test_rpc_node(suite: &ConsensusIntegrationTestSuite) -> anyhow::Result<()> {
    it_info_print!("Running rpc node test");
    let mut test_fed_members = suite.local_context.poa_nodes.as_ref().unwrap().clone();
    // Remove syncing nodes: syncing nodes are the last entries in the map
    let num_to_keep = test_fed_members.len()
        - suite.local_context.cometbft_nodes_syncing.clone().unwrap_or_default().len();
    test_fed_members = test_fed_members.into_iter().take(num_to_keep).collect();
    it_info_print!("Test federation members", test_fed_members.len());

    // create botanix clients
    let mut botanix_clients: Vec<BotanixEthClient> = vec![];
    for (index, fed_member_config) in test_fed_members.iter() {
        botanix_clients.push(
            fed_member_config
                .botanix_eth_client
                .clone()
                .expect("Botanix Client must be initialized"),
        );
        it_info_print!("Botanix client created for poa member {}", index);
    }

    // send eoa messages from all botanix clients
    let eoa_receiver = ethers::core::types::Address::random();

    let last_tx_hash = botanix_clients
        .first()
        .unwrap()
        .send_eoa(eoa_receiver, SEND_AMOUNT)
        .await
        .unwrap()
        .unwrap();
    it_info_print!("Eoa tx to Poa node: {:?}", last_tx_hash);

    // create rpc node and sync with federation peers
    let rpc_node = suite
        .local_context
        .rpc_nodes
        .as_ref()
        .map(|rpc| rpc.get(&0))
        .flatten()
        .cloned()
        .expect("first rpc node to be valid");
    // get latest header hash from rpc node
    // Note: alternative way is to wait for cannon state notification from rpc node and get hash
    // from notification but this way also tests that rpc node can handle rpc requests
    let rpc_botanix_client = BotanixEthClient::new(
        rpc_node.rpc_port,
        rpc_node.ws_port,
        PREFUNDED_ACCOUNT_SECRET_KEY,
        ethers::core::types::Address::random(),
    )
    .await
    .context("Failed to create rpc botanix client")?;

    // get the latest header hash from the federation
    let fed_latest_header_hash = botanix_clients
        .first()
        .expect("botanix client to exist")
        .get_latest_block_hash()
        .await
        .unwrap();
    let rpc_latest_block_header = rpc_botanix_client.get_latest_block_hash().await.unwrap();

    it_info_print!("Federation latest header hash", fed_latest_header_hash);
    it_info_print!("RPC node latest header hash", rpc_latest_block_header);

    assert_eq!(rpc_latest_block_header, fed_latest_header_hash);

    // submit a tx to the rpc node
    let rpc_tx_receipt =
        rpc_botanix_client.send_eoa(eoa_receiver, SEND_AMOUNT).await.unwrap().unwrap();
    it_info_print!("RPC node tx receipt", rpc_tx_receipt);

    // assert tx is confirmed (status = 1)
    let status = rpc_tx_receipt.status.expect("tx status to exist");
    assert_eq!(status, U64::from(1));

    let rpc_tx_block_hash = rpc_tx_receipt.block_hash.expect("block hash to exist");
    it_info_print!("RPC tx block hash", rpc_tx_block_hash);

    // call all fed members and check they have the block with the rpc tx
    for client in botanix_clients.iter() {
        let block = client.get_latest_block_by_hash(rpc_tx_block_hash).await.unwrap();
        let tx_hash = block.transactions.first().expect("tx to exist");
        it_info_print!("Fed node tx hash", tx_hash);
        it_info_print!("RPC node tx hash", rpc_tx_receipt.transaction_hash);

        assert_eq!(*tx_hash, rpc_tx_receipt.transaction_hash);
    }

    Ok(())
}
