use bitcoin::Amount;
use reth_primitives::botanix::utils::AmountExt;

use crate::{it_info_print, suite::consensus::ConsensusIntegrationTestSuite};

pub const SEND_AMOUNT: u64 = 1; // = 1 ether

#[allow(clippy::too_many_lines)]
pub async fn invalid_pegout(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), super::error::InvalidTransactionError> {
    let test_fed_members = suite.local_context.poa_nodes.as_ref().unwrap().clone();

    // subscribe to notifications so channel stays open
    let _rx = suite.local_context.poa_notification.as_ref().expect("poa notifs").subscribe();

    // Generate and send pegout tx
    // invalid bitcoin address
    let mut botanix_eth_client = test_fed_members
        .get(&0)
        .cloned()
        .unwrap()
        .botanix_eth_client
        .clone()
        .expect("Botanix Client must be initialized");

    // create contract deployer to avoid any nonce issues during contract deployment
    let contract_deployer =
        botanix_eth_client.get_contract_deployer().expect("To get contract deployer");

    // Fund the contract deployer
    let _tx_receipt = botanix_eth_client
        .send_eoa(contract_deployer.address(), SEND_AMOUNT)
        .await
        .expect("To send eoa")
        .expect("To get tx receipt");

    // Deploy attack contract
    let attack_contract_address = botanix_eth_client
        .deploy_mint_attack_contract(contract_deployer)
        .await
        .expect("To deploy attack contract");
    botanix_eth_client.set_mint_attack_contract(attack_contract_address);

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

    // use empty pegout data
    let pegout_data = ethers::core::types::Bytes::new();
    let pegout_amount = Amount::from_btc(0.5).unwrap();
    let expected_amount = (pegout_amount / 2).to_wei();
    it_info_print!("Pegout amount: ", pegout_amount.to_wei());

    // send to attack contract which halfs the pegout amount
    // 0.5 BTC -> 0.25 BTC: trying to get refunded 0.5 instead of 0.25 that is burned
    let tx_receipt = botanix_eth_client
        .burn_attack(invalid_pegout_destination, pegout_data, pegout_amount.to_wei())
        .await
        .unwrap()
        .unwrap();

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
