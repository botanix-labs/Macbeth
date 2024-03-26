use bitcoin::{merkle_tree::PartialMerkleTree, Amount};
use ethers::{
    providers::{Http, ProviderError},
    types::U256,
};
use reth::core::cli::runner::CliRunner;
use reth_botanix_lib::peg_contract::{PeginData, PeginMeta};
use reth_primitives::{Account, Address};
use std::{str::FromStr, time::Duration};

use crate::suite::consensus::{
    poa::poa_node::{create_poa_federation_members, Notifications},
    ConsensusIntegrationTestSuite,
};
use ethers::prelude::Provider;

use super::poa_node::is_dkg_ready;
use bitcoincore_rpc::{Auth, RawTx, RpcApi};

const BITCOIND_WALLET_NAME: &str = "botanix_integration_test_wallet";

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct GatewayAddressResponse {
    gateway_address: String,
    aggregate_public_key: String,
    eth_address: String,
}

pub async fn poa_frost_dkg(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), super::error::Error> {
    // Set up regtest connection
    // config is hardcoded to only work with regtest
    let bitcoin_rpc = bitcoincore_rpc::Client::new(
        "localhost:18443",
        Auth::UserPass(
            suite.config.bitcoind.username.clone(),
            suite.config.bitcoind.password.clone(),
        ),
    )
    .expect("bitcoind client");

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

    // Load up the bitcoin wallet and generate some blocks
    let create_res = bitcoin_rpc.create_wallet(BITCOIND_WALLET_NAME, None, None, None, None);
    if create_res.is_err() {
        // wallet already exists
        // load wallet
        let _ = bitcoin_rpc.load_wallet(BITCOIND_WALLET_NAME);
    }
    let address =
        bitcoin_rpc.get_new_address(None, None).expect("get new address").assume_checked();
    // generate some blocks
    bitcoin_rpc.generate_to_address(50, &address).expect("generate to address");

    // Set up dummy eth address
    let eth_address: [u8; 20] = [0; 20];
    // Provider to one of the federation members
    let provider = Provider::<Http>::try_from(
        format!("http://localhost:{}", test_fed_members.get(&0).unwrap().rpc_port).as_str(),
    )
    .expect("could not instantiate HTTP Provider");

    let gateway_address_response = provider
        .request::<Vec<String>, GatewayAddressResponse>(
            "eth_getGatewayAddress",
            vec![hex::encode(eth_address)],
        )
        .await
        .expect("should get gateway address");

    // Send some bitcoin to that gateway address
    let btc_address = bitcoin::Address::from_str(gateway_address_response.gateway_address.as_str())
        .expect("valid btc_address")
        .assume_checked();
    let tx = bitcoin_rpc
        .send_to_address(&btc_address, Amount::ONE_BTC, None, None, Some(true), None, Some(1), None)
        .expect("valid send");
    bitcoin_rpc.generate_to_address(2, &address).expect("generate to address");

    // retrieve the transaction
    let tx_res = bitcoin_rpc.get_transaction(&tx, None).expect("valid tx");
    let tx = tx_res.transaction().expect("valid tx");

    let eth_account = Address::from_slice(&eth_address);
    let vout = 1 as usize;
    let amount = U256::from(tx.output[vout].value);

    // get block headers
    let mut block_headers = vec![];

    // let txids = bitcoin_rpc.get_transaction(52).expect("valid txids");
    let pmt = PartialMerkleTree::from_txids(&[tx.txid()], &[true]);
    let meta = PeginMeta {
        version: 0,
        outpoint: bitcoin::OutPoint::new(tx.txid(), vout as u32),
        address: eth_account,
        aggregate_publickey: bitcoin::secp256k1::PublicKey::from_str(
            gateway_address_response.aggregate_public_key.as_str(),
        )
        .expect("valid public key"),
        tx,
        merkle_proof: pmt,
        block_headers,
    };
    println!("Transaction: {:?}", tx);
    let pegin_proof = PeginData {
        account: eth_account,
        amount,
        bitcoin_block_height: 52,
        meta: vec![meta],
    };
    

    // let block = provider.get_block(100u64).await?;
    // TODO: btc rpc
    // smart contract call etc.

    Ok(())
}
