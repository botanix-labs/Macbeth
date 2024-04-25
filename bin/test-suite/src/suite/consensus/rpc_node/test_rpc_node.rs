use rand::seq::index;
use reth::core::cli::runner::CliRunner;
use reth::primitives::{constants::BOTANIX_FEES_RECIPIENT, public_key_to_address, B256};
use reth_botanix_lib::extra_data_header::ExtraDataHeader;
use std::{collections::HashSet, time::Duration};

use crate::suite::consensus::frost::botanix_client::BotanixEthClient;
use crate::suite::consensus::rpc_node::{
    error::NonFederationMemberTestConfigError, rpc_node::create_rpc_node,
};
use crate::{
    it_info_print,
    suite::consensus::{
        frost::{
            poa_node::{create_poa_federation_members, is_inturn, Notifications},
            test_frost_e2e::await_dkg,
        },
        ConsensusIntegrationTestSuite,
    },
};

use super::rpc_node::NonFederationMemberTestConfig;

const SEND_AMOUNT: u64 = 1; // = 1 Botanix BTC

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
    for (index, fed_member_config) in test_fed_members.iter() {
        let fed_member_config = fed_member_config.clone();
        let _ = std::thread::spawn(move || {
            let fed_member_command = fed_member_config.build_command();
            let runner = CliRunner::default();
            runner.run_command_until_exit(|ctx| fed_member_command.execute(ctx)).unwrap();
        });
        // wait for one second inbetween members start
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // create botanix clients
    let mut botanix_clients: Vec<BotanixEthClient> = vec![];
    for (index, fed_member_config) in test_fed_members.iter() {
        let botanix_eth_client = fed_member_config.create_botanix_eth_client().await;
        botanix_clients.push(botanix_eth_client);
        it_info_print!("Botanix client created for poa member {}", index);
    }
    // wait for the dkg to finish for each of them
    await_dkg(&mut test_fed_members, &mut rx).await;

    // send eoa messages to poa nodes when inturn
    let total_authorities = test_fed_members.len();
    let mut tx_hashes_set = HashSet::new();
    for (index, botanix_eth_client) in botanix_clients.iter().enumerate() {
        'inner: loop {
            let is_test_fed_member_inturn = is_inturn(total_authorities as u64, index as u64);
            it_info_print!("Is in turn?", is_test_fed_member_inturn);
            if is_test_fed_member_inturn {
                it_info_print!("Sending eoa transaction to poa member {}", index);
                let eoa_receiver = ethers::core::types::Address::random();
                let last_tx_hash =
                    botanix_eth_client.send_eoa(eoa_receiver, SEND_AMOUNT).await.unwrap().unwrap();
                it_info_print!("Eoa tx: {:?}", last_tx_hash);
                tx_hashes_set.insert(last_tx_hash.transaction_hash);
                break 'inner;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
    }

    // wait until all block notifications have been received
    let mut fed_latest_header_hash = B256::from([0; 32]);
    while let Some(notification) = rx.recv().await {
        match notification {
            Notifications::CanonState(canon_state_notification) => {
                it_info_print!(
                    "Received payload from engine index",
                    canon_state_notification.engine_index
                );

                if canon_state_notification.engine_index == (total_authorities as u16) - 1 {
                    it_info_print!("Received all canon state notifications");

                    fed_latest_header_hash = canon_state_notification.notification.tip().hash();
                    it_info_print!("Federation latest header hash: {:?}", fed_latest_header_hash);
                    break;
                }
            }
            _ => {}
        }
    }

    // create rpc node and sync with federation peers
    let (rpc_node, mut rx) = create_rpc_node(suite.global_context.clone(), test_fed_members).await;

    // assert non fed node has synced with the federation
    while let Some(notification) = rx.recv().await {
        match notification {
            Notifications::CanonState(canon_state_notification) => {
                it_info_print!(
                    "Received payload from engine index",
                    canon_state_notification.engine_index
                );
                assert_eq!(canon_state_notification.engine_index, total_authorities as u16 + 1);

                let header_hash = canon_state_notification.notification.tip().hash();
                it_info_print!("New non fed member header hash: {:?}", header_hash);
                assert_eq!(header_hash, fed_latest_header_hash);

                break;
            }
            _ => {}
        }
    }

    Ok(())
}
