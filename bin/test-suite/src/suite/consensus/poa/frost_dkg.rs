use reth::core::cli::runner::CliRunner;
use std::time::Duration;

use crate::suite::consensus::{
    poa::poa_node::{create_poa_federation_members, Notifications},
    ConsensusIntegrationTestSuite,
};

use super::poa_node::is_dkg_ready;

pub async fn poa_frost_dkg(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), super::error::Error> {
    // generate test fed members poa nodes
    let (mut test_fed_members, mut rx) =
        create_poa_federation_members(&suite.config, suite.local_context.btc_servers.as_ref())
            .await;

    // run all poa nodes in the background
    for (_index, fed_member_config) in test_fed_members.iter() {
        let fed_member_config = fed_member_config.clone();
        let _ = std::thread::spawn(move || {
            let fed_member_command = fed_member_config.build_command();
            let runner = CliRunner::default();
            runner.run_command_until_exit(|ctx| fed_member_command.execute(ctx)).unwrap();
        });
        // wait for one second inbetween members start
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // wait for the dkg to finish for each of them
    while let Some(notification) = rx.recv().await {
        match notification {
            Notifications::DkgFinished(dkg_notification) => {
                if let Some(fed_member) =
                    test_fed_members.get_mut(&(dkg_notification.engine_index as usize))
                {
                    fed_member.is_dkg_ready = true;
                }
                if is_dkg_ready(&test_fed_members) {
                    break;
                }
            }
            _ => {}
        }
    }

    let _minter_instance_member_1 =
        test_fed_members.get(&0).cloned().unwrap().create_mint_contract_instance().await;
    let _minter_instance_member_2 =
        test_fed_members.get(&1).cloned().unwrap().create_mint_contract_instance().await;

    // TODO: gateway address
    // TODO: btc rpc
    // smart contract call etc.

    Ok(())
}
