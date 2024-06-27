use ethers::types::U64;
use tokio::time::Duration;

use crate::{
    it_info_print,
    suite::consensus::{
        common::{
            botanix_client::BotanixEthClient, events::SEND_AMOUNT,
            poa_node::PREFUNDED_ACCOUNT_SECRET_KEY,
        },
        rpc_node::error::NonFederationMemberTestConfigError,
        ConsensusIntegrationTestSuite,
    },
};

#[allow(clippy::unwrap_used, clippy::cast_possible_truncation)]
pub async fn test_rpc_node(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), NonFederationMemberTestConfigError> {
    it_info_print!("Running rpc node test");
    // subscribe to rpc node events
    let rx = suite
        .local_context
        .rpc_notification
        .as_ref()
        .expect("broadcast sender to be set")
        .subscribe();
    let test_fed_members = suite.local_context.poa_nodes.as_ref().unwrap();
    // create botanix clients
    let mut botanix_clients: Vec<BotanixEthClient> = vec![];
    for (index, fed_member_config) in test_fed_members.iter() {
        let botanix_eth_client = fed_member_config.create_botanix_eth_client().await;
        botanix_clients.push(botanix_eth_client);
        it_info_print!("Botanix client created for poa member {}", index);
    }

    // send eoa messages from all botanix clients
    let total_authorities = test_fed_members.len();
    let eoa_receiver = ethers::core::types::Address::random();
    // build a block for each fed member

    // it_info_print!("Sending eoa transaction to poa member", inturn_member_index);
    let last_tx_hash = botanix_clients
        .first()
        .unwrap()
        .send_eoa(eoa_receiver, SEND_AMOUNT)
        .await
        .unwrap()
        .unwrap();
    it_info_print!("Eoa tx to Poa node: {:?}", last_tx_hash);

    // get the latest header hash from the federation
    let fed_latest_header_hash = botanix_clients
        .first()
        .expect("botanix client to exist")
        .get_latest_block_hash()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(10)).await;
    // create rpc node and sync with federation peers
    let rpc_node = suite.local_context.rpc_node.as_ref().unwrap();
    // get latest header hash from rpc node
    // Note: alternative way is to wait for cannon state notification from rpc node and get hash
    // from notification but this way also tests that rpc node can handle rpc requests
    let rpc_botanix_client = BotanixEthClient::new(
        rpc_node.rpc_port,
        PREFUNDED_ACCOUNT_SECRET_KEY,
        ethers::core::types::Address::random(),
    )
    .await;

    let rpc_latest_block_header = rpc_botanix_client.get_latest_block_hash().await.unwrap();
    it_info_print!("RPC node latest header hash", rpc_latest_block_header);

    assert_eq!(rpc_latest_block_header, fed_latest_header_hash);

    // submit a tx to the rpc node
    let rpc_tx_receipt =
        rpc_botanix_client.send_eoa(eoa_receiver, SEND_AMOUNT).await.unwrap().unwrap();
    it_info_print!("RPC node tx hash", rpc_tx_receipt);

    // assert tx is confirmed (status = 1)
    let status = rpc_tx_receipt.status.expect("tx status to exist");
    assert_eq!(status, U64::from(1));

    // wait for fed members to sync on the block
    tokio::time::sleep(Duration::from_secs(5)).await;

    let latest_block_hash = rpc_tx_receipt.block_hash.expect("block hash to exist");

    // call all fed members and check they have same block hash
    // then check block contains rpc tx
    for (index, client) in botanix_clients.iter().enumerate() {
        let block_hash = client.get_latest_block_hash().await.unwrap();
        it_info_print!("Botanix client", format!("index={index}: block hash - {block_hash}"));

        assert_eq!(block_hash, latest_block_hash);
        // if last fed member, check tx is in block
        if index == total_authorities - 1 {
            let block = client.get_latest_block_by_hash(block_hash).await.unwrap();
            it_info_print!("Final Botanix client block", block);

            let tx_hash = block.transactions.first().expect("tx to exist");
            it_info_print!("Latest block containing tx hash", tx_hash);
            it_info_print!("RPC node tx hash", rpc_tx_receipt.transaction_hash);

            assert_eq!(*tx_hash, rpc_tx_receipt.transaction_hash);
        }
    }

    Ok(())
}
