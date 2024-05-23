use ethers::types::U64;
use reth::{
    consensus_common::utils::{current_inturn_index, unix_timestamp},
    core::cli::runner::CliRunner,
    primitives::ChainSpec,
};
use std::time::Duration;

use crate::{
    it_info_print,
    suite::consensus::{
        common::{
            botanix_client::BotanixEthClient,
            events::{await_dkg, SEND_AMOUNT},
            poa_node::{create_poa_federation_members, PREFUNDED_ACCOUNT_SECRET_KEY},
            rpc_node::create_rpc_node,
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

    // generate test fed members poa nodes
    let (mut test_fed_members, mut fed_rx) = create_poa_federation_members(
        suite.global_context.clone(),
        suite.local_context.btc_servers.as_ref(),
    )
    .await;

    // run all poa nodes in the background
    let mut chain_spec = ChainSpec::default();
    for (_index, fed_member_config) in test_fed_members.iter() {
        let (fed_member_command, spec) = fed_member_config.build_command();
        chain_spec = spec;
        let _ = std::thread::spawn(move || {
            let runner = CliRunner::default();
            runner.run_command_until_exit(|ctx| fed_member_command.execute(ctx)).unwrap();
        });
        // wait for one second inbetween members start
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // wait for the dkg to finish for each of them
    await_dkg(&mut test_fed_members, &mut fed_rx).await;

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
    for index in 0..total_authorities {
        let inturn_member_index = current_inturn_index(total_authorities as u64, unix_timestamp());

        it_info_print!("Sending eoa transaction to poa member", inturn_member_index);
        let last_tx_hash = botanix_clients[inturn_member_index as usize]
            .send_eoa(eoa_receiver, SEND_AMOUNT)
            .await
            .unwrap()
            .unwrap();
        it_info_print!("Eoa tx: {:?}", last_tx_hash);

        // wait for next member to come inturn but not for the final member
        if index < total_authorities - 1 {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    }
    it_info_print!("All eoa transactions sent");

    // get the latest header hash from the federation
    let fed_latest_header_hash = botanix_clients
        .first()
        .expect("botanix client to exist")
        .get_latest_block_hash()
        .await
        .unwrap();

    // create rpc node and sync with federation peers
    let (rpc_node, _rx) = create_rpc_node(suite.global_context.clone(), test_fed_members).await;
    let rpc_node_clone = rpc_node.clone();
    let _ = std::thread::spawn(move || {
        let rpc_node_command = rpc_node_clone.build_command(chain_spec);
        let runner = CliRunner::default();
        match runner.run_command_until_exit(|ctx| rpc_node_command.execute(ctx)) {
            Ok(()) => it_info_print!("RPC node started successfully"),
            Err(e) => it_info_print!("RPC node failed to start", e),
        }
    });

    // wait for rpc node spin up and to sync with the federation
    tokio::time::sleep(Duration::from_secs(15)).await;

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
    tokio::time::sleep(Duration::from_secs(15)).await;

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
