use bitcoin::Amount;
use reth_botanix_lib::utils::AmountExt;

use crate::{it_info_print, suite::consensus::ConsensusIntegrationTestSuite};

#[allow(clippy::too_many_lines)]
pub async fn invalid_pegout(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), super::error::InvalidTransactionError> {
    let test_fed_members = suite.local_context.poa_nodes.as_ref().unwrap().clone();
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
