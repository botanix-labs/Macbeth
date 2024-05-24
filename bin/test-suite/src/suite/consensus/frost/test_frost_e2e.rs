use std::{str::FromStr, time::Duration};

use bitcoin::{hashes::Hash, merkle_tree::PartialMerkleTree, Amount};
use bitcoincore_rpc::{Auth, RpcApi};
use ethers::{
    prelude::Provider,
    providers::{Http, Middleware},
    types::{NameOrAddress, U256},
};
use reth_botanix_lib::{
    mint_validation::{BURN_TOPIC, MINT_TOPIC},
    peg_contract::{PeginData, PeginMeta},
    utils::AmountExt,
};
use reth_btc_wallet::address::EthAddress;
use reth_cli_runner::CliRunner;
use reth_primitives::Address;

use crate::{
    it_info_print,
    suite::consensus::{
        common::{
            events::{
                await_botanix_event, await_dkg, GatewayAddressResponse, BITCOIND_WALLET_NAME,
                SEND_AMOUNT,
            },
            poa_node::create_poa_federation_members,
        },
        ConsensusIntegrationTestSuite,
    },
};

pub async fn frost_e2e_stable(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), super::error::Error> {
    // Set up regtest connection
    // config is hardcoded to only work with regtest
    let bitcoind_rpc = suite.global_context.bitcoind_rpc();

    // Load up the bitcoin wallet and generate some blocks
    for wallet in bitcoind_rpc.list_wallets().unwrap() {
        it_info_print!("#UNLOADING WALLET?", &wallet);
        let _ = bitcoind_rpc.unload_wallet(Some(&wallet));
    }
    let create_res = bitcoind_rpc.create_wallet(BITCOIND_WALLET_NAME, None, None, None, None);
    if create_res.is_err() {
        // wallet already exists
        // load wallet
        let _ = bitcoind_rpc.load_wallet(BITCOIND_WALLET_NAME);
    }
    let address =
        bitcoind_rpc.get_new_address(None, None).expect("get new address").assume_checked();
    // generate some blocks so the wallet has a non-zero balance
    bitcoind_rpc.generate_to_address(10, &address).expect("generate to address");

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

    // generate mint contract test instances
    let mut mint_contract_instances = Vec::new();
    for (index, _) in test_fed_members.iter() {
        let botanix_eth_client =
            test_fed_members.get(index).cloned().unwrap().create_botanix_eth_client().await;
        mint_contract_instances.push(botanix_eth_client);
    }

    // Set up dummy eth address
    let eth_destination = ethers::core::types::Address::random();

    // Provider to one of the federation members
    let provider = Provider::<Http>::try_from(format!(
        "http://localhost:{}",
        test_fed_members.get(&0).unwrap().rpc_port
    ))
    .expect("could not instantiate HTTP Provider");

    // get gateway address
    let gateway_address_response = provider
        .request::<Vec<String>, GatewayAddressResponse>(
            "eth_getGatewayAddress",
            vec![hex::encode(eth_destination.0)],
        )
        .await
        .expect("should get gateway address");

    it_info_print!("Gateway Address Response", gateway_address_response);

    // print balance
    let balance = bitcoind_rpc.get_balance(None, None).expect("get balance");
    it_info_print!("Bitcoin balance", balance);

    // Send some bitcoin to that gateway address
    let btc_address = bitcoin::Address::from_str(gateway_address_response.gateway_address.as_str())
        .expect("valid btc_address")
        .assume_checked();
    let pegin_txid = bitcoind_rpc
        .send_to_address(&btc_address, Amount::ONE_BTC, None, None, Some(true), None, Some(1), None)
        .expect("valid send");
    // Generate some block to confirm it
    bitcoind_rpc.generate_to_address(
        2 + reth_primitives::constants::MAINNET_PEGIN_CONFIRMATION_DEPTH as u64, &address,
    ).expect("generate to address");
    tokio::time::sleep(Duration::from_secs(5)).await;

    // retrieve the transaction
    let tx_res = bitcoind_rpc.get_transaction(&pegin_txid, None).expect("valid tx");
    assert!(tx_res.info.confirmations > 1);
    let pegin_tx = tx_res.transaction().expect("valid tx");
    it_info_print!("Bitcoin pegin Tx", pegin_tx);
    it_info_print!("Gateway Data", gateway_address_response);
    it_info_print!("Gateway Data Pub key", gateway_address_response.aggregate_public_key);

    let eth_account = Address::from_slice(eth_destination.as_slice());
    let (vout, pegin_output) = pegin_tx
        .output
        .iter()
        .enumerate()
        .find(|(_, o)| o.script_pubkey == btc_address.script_pubkey())
        .unwrap();
    let amount = pegin_output.value.to_wei();
    it_info_print!("Btc Amount", amount);

    // get block headers
    // first we need the block hash of the block with the conf'd pegin tx
    let conf_hash = tx_res.info.blockhash.expect("pegin confirmed");
    let tip = bitcoind_rpc.get_best_block_hash().unwrap();
    it_info_print!("Bitcoin Chain Tip", tip);
    let tip_header = bitcoind_rpc.get_block_header(&tip).expect("valid block header");
    // We will collect all the headers all the way up to the tip which is not needed, but allowed.
    // In theory, we only need to collect headers from the block our pegin is in, to the finalized
    // block (the one in the mainchain commitment).
    let mut headers = vec![];
    let mut cursor = tip_header;
    let mut stopgap = 200; // just to make sure we don't infinite loop until genesis
    loop {
        stopgap -= 1;
        if stopgap == 0 || cursor.prev_blockhash == bitcoin::BlockHash::all_zeros() {
            panic!("confirmation block not found...");
        }

        headers.push(cursor);
        if cursor.block_hash() == conf_hash {
            break;
        }
        cursor = bitcoind_rpc.get_block_header(&cursor.prev_blockhash).unwrap();
    }
    headers.reverse();
    it_info_print!("Number of pegin_headers: {}", headers.len());

    let conf_block_info = bitcoind_rpc.get_block_info(&conf_hash).expect("valid txids");
    it_info_print!("Block info", conf_block_info);
    let pmt = PartialMerkleTree::from_txids(&conf_block_info.tx, &[false, true]);

    // create pegin meta
    let bitcoin_block_height = conf_block_info.height;
    let meta = PeginMeta {
        version: 0,
        outpoint: bitcoin::OutPoint::new(pegin_tx.txid(), vout as u32),
        address: eth_account,
        aggregate_publickey: bitcoin::secp256k1::PublicKey::from_str(
            gateway_address_response.aggregate_public_key.as_str(),
        )
        .expect("valid public key"),
        tx: pegin_tx.clone(),
        merkle_proof: pmt,
        block_headers: headers,
    };

    // validate the pegin data first offchain before submitting
    let pegin_data = PeginData {
        account: Address::from_slice(eth_destination.as_bytes()),
        amount: amount,
        bitcoin_block_height: bitcoin_block_height as u32,
        meta: vec![meta.clone()],
    };
    let finalized = {
        let tip = bitcoind_rpc.get_block_count().unwrap();
        let height = tip - reth_primitives::constants::MAINNET_PEGIN_CONFIRMATION_DEPTH as u64;
        let hash = bitcoind_rpc.get_block_hash(height).unwrap();
        (bitcoind_rpc.get_block_header(&hash).unwrap(), height as u32)
    };
    pegin_data.validate(
        &finalized,
        &bitcoin::secp256k1::PublicKey::from_str(
            gateway_address_response.aggregate_public_key.as_str(),
        ).unwrap(),
    ).expect("pegin data should be invalid!");
    it_info_print!("Pegindata successfully validated");

    // send the pegin transactions to all fed memebers
    it_info_print!("Sending pegin tx: block headers={:?}",
        meta.block_headers.iter().map(|h| h.block_hash()).collect::<Vec<_>>()
    );
    let serialized_pegin_meta = meta.serialize();
    it_info_print!("Serialized pegin meta: ", hex::encode(serialized_pegin_meta.clone()));
    let mint_contract = mint_contract_instances.first().cloned().unwrap();
    let metadata = ethers::core::types::Bytes::from(serialized_pegin_meta.clone());
    let tx_receipt = mint_contract
        .mint(
            eth_destination.clone(),
            amount,
            bitcoin_block_height as u32,
            metadata,
            ethers::core::types::Address::random(),
        )
        .await
        .unwrap();
    it_info_print!("Mint Tx Receipt ", tx_receipt);

    // wait for a few blocks to make sure the tx got included and mined
    it_info_print!("Waiting for botanix event after mint call");
    await_botanix_event(&mut rx, *MINT_TOPIC).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // make sure we have received the botanix btc on botanix
    let eth_address = NameOrAddress::from_str(&eth_account.to_string()).unwrap();
    let eth_address_balance = provider.get_balance(eth_address, None).await.unwrap();
    assert!(!eth_address_balance.is_zero());

    // Generate and send pegout tx
    // bitcoin address
    let pegout_destination =
        ethers::core::types::Bytes::from(btc_address.to_string().as_bytes().to_vec());
    // use empty pegout data
    let pegout_data = ethers::core::types::Bytes::new();
    let pegout_amount = Amount::from_btc(0.5).unwrap();
    let tx_receipt =
        mint_contract.burn(pegout_destination, pegout_data, pegout_amount.to_wei()).await.unwrap();
    it_info_print!("Pegout Tx Receipt: ", tx_receipt);

    // wait for the tx to be included in a botanix block
    await_botanix_event(&mut rx, *BURN_TOPIC).await;

    // make sure we have enough time for the nonce to be updated
    tokio::time::sleep(Duration::from_secs(10)).await;

    // need another tx to enter an epoch
    let eoa_tx_receipt =
        mint_contract.send_eoa(ethers::core::types::Address::random(), SEND_AMOUNT).await.unwrap();
    it_info_print!("Eoa Tx Receipt: ", eoa_tx_receipt);

    // sleep for a few more seconds
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Reconnect to bitcoind. Occasionally the connection is lost after a long time or b/c of other
    // processes connecting
    let bitcoind_rpc = suite.global_context.bitcoind_rpc();
    // mine some btc blocks (needed for confirmed pegout)
    bitcoind_rpc.generate_to_address(1, &address).expect("generate to address");

    // Retrieve the last block
    let tip_hash = bitcoind_rpc.get_best_block_hash().expect("valid block hash");
    let tip_block = bitcoind_rpc.get_block(&tip_hash).expect("valid block");
    // there should be 2 transaction one of which is the pegout the other is coinbase
    assert_eq!(tip_block.txdata.len(), 2);
    let pegout_tx = tip_block.txdata.get(1).unwrap();
    it_info_print!("Pegout tx: ", pegout_tx);

    assert_eq!(pegout_tx.input.len(), 1);
    assert_eq!(pegout_tx.input[0].previous_output.txid, pegin_tx.txid());
    assert_eq!(pegout_tx.input[0].previous_output.vout, vout as u32);
    assert_eq!(pegout_tx.output.len(), 2);
    // One of the values here should be the pegout address
    let mut match_found = false;
    for output in pegout_tx.output.iter() {
        let pegout_address = output.script_pubkey.clone();
        let address_spk = btc_address.script_pubkey();
        match_found = pegout_address == address_spk;
        if match_found {
            break;
        }
    }
    assert!(match_found);
    // TODO We could do a percise amounts check here
    assert!(pegout_tx.output[1].value > Amount::from_sat(0));

    Ok(())
}
