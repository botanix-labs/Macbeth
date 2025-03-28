use bitcoin::Amount;
use reth_primitives::botanix::utils::AmountExt;

use crate::{it_info_print, suite::consensus::ConsensusIntegrationTestSuite};

#[allow(clippy::too_many_lines)]
pub async fn invalid_pegout(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), super::error::InvalidTransactionError> {
    let test_fed_members = suite.local_context.poa_nodes.as_ref().unwrap().clone();

    // subscribe to notifications so channel stays open
    let _rx = suite.local_context.poa_notification.as_ref().expect("poa notifs").subscribe();

    // Generate and send pegout tx
    // invalid bitcoin address
    let botanix_eth_client = test_fed_members
        .get(&0)
        .cloned()
        .unwrap()
        .botanix_eth_client
        .clone()
        .expect("Botanix Client must be initialized");
    let invalid_pegout_destination = ethers::core::types::Bytes::from(
        "invalid_pegout_destination".to_string().as_bytes().to_vec(),
    );

    let sender_address = botanix_eth_client.get_sender_address();
    // sender address balance before pegout
    let mut sender_address_initial_balance = botanix_eth_client
        .get_botanix_balance(reth_primitives::Address(
            botanix_eth_client.get_sender_address().0.into(),
        ))
        .await
        .unwrap();
    it_info_print!("Sender address initial balance: ", sender_address_initial_balance);

    // nonce before pegout
    let nonce_before =
        botanix_eth_client.get_nonce(botanix_eth_client.get_sender_address()).await.unwrap();
    it_info_print!("Nonce before pegout: ", nonce_before);

    // Lets also test that when pegouts fail, the reverted amount
    // is the actual burned amount. For this we need the burned amount
    // to be different from the pegout amount. 
    let burned_amount = Amount::from_btc(0.1).unwrap();   // the "true" burned amount
    let pegout_amount = Amount::from_btc(0.5).unwrap();  

    // Craft pegout_data such that it encodes the small burned amount.
    let encoded_amount = ethers::abi::Token::Uint(
        ethers::types::U256::from(burned_amount.to_wei().as_u64()),
    );
    let encoded_destination = ethers::abi::Token::String("invalid_pegout_destination".to_string());
    let encoded_version = ethers::abi::Token::Bytes(vec![0]);
    let pegout_data = ethers::core::types::Bytes::from(
        ethers::abi::encode(&[encoded_amount, encoded_destination, encoded_version])
    );
    
    it_info_print!("Pegout amount: ", pegout_amount.to_wei());
    it_info_print!("Burned amount: ", burned_amount.to_wei());
    let tx_receipt = botanix_eth_client
        .burn(invalid_pegout_destination, pegout_data, pegout_amount.to_wei())
        .await
        .unwrap()
        .unwrap();
    it_info_print!("Pegout Tx Receipt: ", tx_receipt);

    assert!(tx_receipt.status.unwrap().is_zero());

    // sender address balance after pegout
    let sender_address_final_balance = botanix_eth_client
        .get_botanix_balance(reth_primitives::Address(
            botanix_eth_client.get_sender_address().0.into(),
        ))
        .await
        .unwrap();
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
