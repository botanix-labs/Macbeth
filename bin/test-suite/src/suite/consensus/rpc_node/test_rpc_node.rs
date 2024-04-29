use reth::core::cli::runner::CliRunner;
use reth::primitives::ChainSpec;
use std::time::Duration;

use crate::suite::consensus::frost::botanix_client::BotanixEthClient;
use crate::suite::consensus::frost::poa_node::current_inturn_index;
use crate::suite::consensus::rpc_node::{
    error::NonFederationMemberTestConfigError, rpc_node::create_rpc_node,
};
use crate::{
    it_info_print,
    suite::consensus::{
        frost::{poa_node::create_poa_federation_members, test_frost_e2e::await_dkg},
        ConsensusIntegrationTestSuite,
    },
};

const SEND_AMOUNT: u64 = 1; // = 1 Botanix BTC

#[allow(clippy::unwrap_used, clippy::cast_possible_truncation)]
pub async fn test_rpc_node(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), NonFederationMemberTestConfigError> {
    it_info_print!("Running rpc node test");

    // generate test fed members poa nodes
    let (mut test_fed_members, mut rx) = create_poa_federation_members(
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
    await_dkg(&mut test_fed_members, &mut rx).await;

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
    for _ in 0..total_authorities {
        let inturn_member_index = current_inturn_index(total_authorities as u64);

        it_info_print!("Sending eoa transaction to poa member", inturn_member_index);
        let last_tx_hash = botanix_clients[inturn_member_index as usize]
            .send_eoa(eoa_receiver, SEND_AMOUNT)
            .await
            .unwrap()
            .unwrap();
        it_info_print!("Eoa tx: {:?}", last_tx_hash);

        // wait for next member to come inturn
        tokio::time::sleep(Duration::from_secs(60)).await;
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
    tokio::time::sleep(Duration::from_secs(60)).await;

    // get latest header hash from rpc node
    // Note: alternative way is to wait for cannon state notification from rpc node and get hash from notification
    // but this way also tests that rpc node can handle rpc requests
    let rpc_botanix_client = BotanixEthClient::new(
        rpc_node.rpc_port,
        &rpc_node.secret_key,
        ethers::core::types::Address::random(),
    )
    .await;

    let rpc_latest_block_header = rpc_botanix_client.get_latest_block_hash().await.unwrap();
    it_info_print!("RPC node latest header hash", rpc_latest_block_header);

    assert_eq!(rpc_latest_block_header, fed_latest_header_hash);

    Ok(())
}
