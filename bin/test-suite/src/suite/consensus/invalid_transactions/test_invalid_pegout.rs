use bitcoin::Amount;
use reth_botanix_lib::utils::AmountExt;
use reth_btc_wallet::address::EthAddress;
use reth_primitives::Address;
use std::time::Duration;

use crate::{it_info_print, suite::consensus::ConsensusIntegrationTestSuite};

#[allow(clippy::too_many_lines)]
pub async fn invalid_pegout(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), super::error::InvalidTransactionError> {
    // wait for everything to spin up otherwise btc_server will hit `Unable to get public key`
    tokio::time::sleep(Duration::from_secs(5)).await;

    let test_fed_members = suite.local_context.poa_nodes.as_ref().unwrap().clone();

    // subscribe to notifications so channel stays open
    let _rx = suite.local_context.poa_notification.as_ref().expect("poa notifs").subscribe();

    // Generate and send pegout tx
    // invalid bitcoin address
    let botanix_eth_client =
        test_fed_members.get(&0).cloned().unwrap().create_botanix_eth_client().await;
    let invalid_pegout_destination = ethers::core::types::Bytes::from(
        "invalid_pegout_destination".to_string().as_bytes().to_vec(),
    );

    let sender_address = botanix_eth_client.get_sender_address();
    let sender_address_string = Address::from_slice(sender_address.as_slice()).to_string();
    // sender address balance before pegout
    let mut sender_address_initial_balance =
        botanix_eth_client.get_botanix_balance(sender_address_string.as_str()).await.unwrap();
    it_info_print!("Sender address initial balance: ", sender_address_initial_balance);

    // nonce before pegout
    let nonce_before = botanix_eth_client.get_nonce(sender_address.clone()).await.unwrap();
    it_info_print!("Nonce before pegout: ", nonce_before);

    // use empty pegout data
    let pegout_data = ethers::core::types::Bytes::new();
    let pegout_amount = Amount::from_btc(0.5).unwrap();
    it_info_print!("Pegout amount: ", pegout_amount.to_wei());
    let tx_receipt = botanix_eth_client
        .burn(invalid_pegout_destination, pegout_data, pegout_amount.to_wei())
        .await
        .unwrap()
        .unwrap();
    it_info_print!("Pegout Tx Receipt: ", tx_receipt);

    assert!(tx_receipt.status.unwrap().is_zero());

    // sender address balance after pegout
    let sender_address_final_balance =
        botanix_eth_client.get_botanix_balance(sender_address_string.as_str()).await.unwrap();
    it_info_print!("Sender address final balance: ", sender_address_final_balance);

    // subtract tx costs from initial balance
    let tx_cost = tx_receipt.gas_used.unwrap() * tx_receipt.effective_gas_price.unwrap();
    it_info_print!("Tx cost: ", tx_cost);
    sender_address_initial_balance -= tx_cost;

    assert_eq!(sender_address_initial_balance, sender_address_final_balance);

    // nonce after pegout
    let nonce_after = botanix_eth_client.get_nonce(sender_address.clone()).await.unwrap();
    it_info_print!("Nonce after pegout: ", nonce_after);

    assert!(nonce_after > nonce_before);

    Ok(())
}
