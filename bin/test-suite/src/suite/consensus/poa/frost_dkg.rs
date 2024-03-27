use bitcoin::{merkle_tree::PartialMerkleTree, Amount};
use ethers::{
    providers::{Http, Middleware, ProviderError},
    types::{NameOrAddress, U256},
};
use reth::core::cli::runner::CliRunner;
use reth_botanix_lib::{
    mint_validation::{BURN_TOPIC, MINT_TOPIC},
    peg_contract::{PeginData, PeginMeta},
};
use reth_btc_wallet::address::EthAddress;
use reth_primitives::{Account, Address, Receipt, B256};
use reth_provider::{chain::BlockReceipts, CanonStateNotification};
use std::{collections::HashMap, str::FromStr, time::Duration};

use crate::suite::consensus::{
    poa::{
        payload_client::PayloadClient,
        poa_node::{create_poa_federation_members, Notifications},
    },
    ConsensusIntegrationTestSuite,
};
use ethers::prelude::Provider;

use super::poa_node::{is_dkg_ready, FederationMemberTestConfig};
use bitcoincore_rpc::{Auth, RawTx, RpcApi};

const BITCOIND_WALLET_NAME: &str = "botanix_integration_test_wallet";
const ETHEREUM_TEST_ADDRESS: &str = "0x184ba627DB853244c9f17f3Cb4378cB8B39bf147";

async fn await_dkg(
    fed_members: &mut HashMap<usize, FederationMemberTestConfig>,
    rx: &mut tokio::sync::mpsc::Receiver<Notifications>,
) {
    while let Some(notification) = rx.recv().await {
        match notification {
            Notifications::DkgFinished(dkg_notification) => {
                if let Some(fed_member) =
                    fed_members.get_mut(&(dkg_notification.engine_index as usize))
                {
                    fed_member.is_dkg_ready = true;
                }
                if is_dkg_ready(&fed_members) {
                    break;
                }
            }
            _ => {}
        }
    }
}

async fn await_botanix_event(
    rx: &mut tokio::sync::mpsc::Receiver<Notifications>,
    event_topic: B256,
) {
    // wait for a few blocks to make sure the tx got included and mined
    while let Some(notification) = rx.recv().await {
        match notification {
            Notifications::CanonState(canon_state_notification) => {
                let block_receipts = canon_state_notification.notification.block_receipts();
                let non_reverted_block_receipts = block_receipts
                    .into_iter()
                    .filter_map(|(receipt, reverted)| if !reverted { Some(receipt) } else { None })
                    .collect::<Vec<BlockReceipts>>();
                let final_block_receipts =
                    non_reverted_block_receipts.into_iter().fold(vec![], |mut acc, receipts| {
                        let receipts = receipts
                            .tx_receipts
                            .into_iter()
                            .filter_map(|(_, r)| if r.success { Some(r) } else { None })
                            .collect::<Vec<Receipt>>();
                        acc.extend(receipts);
                        acc
                    });
                println!("Final block receipts {:?}", final_block_receipts);
                for block_receipt in final_block_receipts.into_iter() {
                    for log in block_receipt.logs.into_iter() {
                        for topic in log.topics.into_iter() {
                            if topic.0 == event_topic.0 {
                                break;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

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
    await_dkg(&mut test_fed_members, &mut rx).await;

    // generate mint contract test instances
    let mut mint_contract_instances = Vec::new();
    for (index, _) in test_fed_members.iter() {
        let minter_instance_member =
            test_fed_members.get(index).cloned().unwrap().create_mint_contract_instance().await;
        mint_contract_instances.push(minter_instance_member);
    }

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
    let eth_destination = ethers::core::types::Address::from_str(ETHEREUM_TEST_ADDRESS).unwrap();

    // Provider to one of the federation members
    let provider = Provider::<Http>::try_from(
        format!("http://localhost:{}", test_fed_members.get(&0).unwrap().rpc_port).as_str(),
    )
    .expect("could not instantiate HTTP Provider");

    // get gateway address
    let gateway_address_response = provider
        .request::<Vec<String>, GatewayAddressResponse>(
            "eth_getGatewayAddress",
            vec![hex::encode(eth_destination.0)],
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

    // generate som btc blocks
    bitcoin_rpc.generate_to_address(2, &address).expect("generate to address");

    // retrieve the transaction
    let tx_res = bitcoin_rpc.get_transaction(&tx, None).expect("valid tx");
    let tx = tx_res.transaction().expect("valid tx");

    let eth_account = Address::from_slice(eth_destination.as_slice());
    let vout = 1;
    let amount = U256::from(tx.output[vout].value);

    // get block headers
    let mut block_headers = vec![];

    // create partial merkle tree
    let pmt = PartialMerkleTree::from_txids(&[tx.txid()], &[false, true]);

    // create pegin meta
    let bitcoin_block_height = 52;
    let meta = PeginMeta {
        version: 0,
        outpoint: bitcoin::OutPoint::new(tx.txid(), vout as u32),
        address: eth_account,
        aggregate_publickey: bitcoin::secp256k1::PublicKey::from_str(
            gateway_address_response.aggregate_public_key.as_str(),
        )
        .expect("valid public key"),
        tx: tx.clone(),
        merkle_proof: pmt,
        block_headers,
    };
    println!("Transaction: {:?}", tx);

    // send the pegin transactions to all fed memebers
    let serialized_pegin_meta = meta.serialize();
    for mint_contract in mint_contract_instances.iter() {
        let metadata = ethers::core::types::Bytes::from(serialized_pegin_meta.clone());
        let tx_receipt = mint_contract
            .mint(eth_destination.clone(), amount, bitcoin_block_height, metadata)
            .await
            .unwrap();
        println!("Tx receipt: {:?}", tx_receipt);
    }

    // wait for a few blocks to make sure the tx got included and mined
    await_botanix_event(&mut rx, *MINT_TOPIC).await;

    // make sure we have received the botanix btc on botanix
    let eth_address = NameOrAddress::from_str(&eth_account.to_string()).unwrap();
    let eth_address_balance = provider.get_balance(eth_address, None).await.unwrap();
    assert_eq!(eth_address_balance, U256::from(Amount::ONE_BTC.to_sat()));

    // send a pegout transactions to all fed memebers
    for mint_contract in mint_contract_instances.iter() {
        let metadata = ethers::core::types::Bytes::from(serialized_pegin_meta.clone());
        // TODO
        let pegout_destination = ethers::core::types::Bytes::new();
        // use empty pegout data
        let pegout_data = ethers::core::types::Bytes::new();
        let tx_receipt = mint_contract.burn(pegout_destination, pegout_data).await.unwrap();
        println!("Tx receipt: {:?}", tx_receipt);
    }

    // wait for the tx to be included in a botanix block
    await_botanix_event(&mut rx, *BURN_TOPIC).await;

    // mine some btc blocks (needed for confirmed pegout)
    bitcoin_rpc.generate_to_address(50, &address).expect("generate to address");

    // TODO: wait for the signing to complete, assertions and check balances

    Ok(())
}
