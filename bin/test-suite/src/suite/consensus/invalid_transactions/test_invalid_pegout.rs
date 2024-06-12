use bitcoin::Amount;
use reth_botanix_lib::utils::AmountExt;
use reth_cli_runner::CliRunner;
use std::time::Duration;

use crate::{
    it_info_print,
    suite::consensus::{
        common::{events::await_dkg, poa_node::create_poa_federation_members},
        ConsensusIntegrationTestSuite,
    },
};

#[allow(clippy::too_many_lines)]
pub async fn invalid_pegout(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), super::error::InvalidTransactionError> {
    // generate test fed members poa nodes
    let (mut test_fed_members, mut rx) = create_poa_federation_members(
        suite.global_context.clone(),
        suite.local_context.btc_servers.as_ref(),
    )
    .await;

    // run all poa nodes in the background
    for (_index, fed_member_config) in test_fed_members.iter() {
        let fed_member_config = fed_member_config.clone();
        let _ = std::thread::spawn(move || {
            let (fed_member_command, _chain_spec) = fed_member_config.build_command();
            let runner = CliRunner::default();
            runner.run_command_until_exit(|ctx| fed_member_command.execute(ctx)).unwrap();
        });
        // wait for one second inbetween members start
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // wait for the dkg to finish for each of them
    await_dkg(&mut test_fed_members, &mut rx).await;

    // Generate and send pegout tx
    // invalid bitcoin address
    let botanix_eth_client =
        test_fed_members.get(&0).cloned().unwrap().create_botanix_eth_client().await;
    let invalid_pegout_destination = ethers::core::types::Bytes::from(
        "invalid_pegout_destination".to_string().as_bytes().to_vec(),
    );
    // use empty pegout data
    let pegout_data = ethers::core::types::Bytes::new();
    let pegout_amount = Amount::from_btc(0.5).unwrap();
    let tx_receipt = botanix_eth_client
        .burn(invalid_pegout_destination, pegout_data, pegout_amount.to_wei())
        .await
        .unwrap()
        .unwrap();
    it_info_print!("Pegout Tx Receipt: ", tx_receipt);

    assert!(tx_receipt.status.unwrap().is_zero());

    Ok(())
}
