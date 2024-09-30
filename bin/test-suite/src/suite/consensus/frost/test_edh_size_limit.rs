use super::error::Error;
use crate::{
    it_info_print,
    suite::consensus::{
        common::events::{
            await_botanix_event, GatewayAddressResponse, BITCOIND_WALLET_NAME, SEND_AMOUNT,
        },
        ConsensusIntegrationTestSuite,
    },
};
use bitcoin::{hashes::Hash, merkle_tree::PartialMerkleTree, Amount};
use bitcoincore_rpc::RpcApi;
use core::panic;
use ethers::{
    prelude::Provider,
    providers::{Http, PendingTransaction},
};
use hex::{self};
use reth_btc_wallet::address::EthAddress;
use reth_chainspec::BOTANIX_TESTNET;
use reth_primitives::{
    botanix::{
        mint_validation::BURN_TOPIC,
        peg_contract::{PeginData, PeginMeta},
        utils::AmountExt,
    },
    Address,
};
use std::{str::FromStr, time::Duration};

const NUM_PEGINS: u16 = 1000;

// NOTE: this test cannot run as currently implemented
// TODO: revisit if test is needed after new consensus implementation

pub async fn test_edh_size_limit(suite: &ConsensusIntegrationTestSuite) -> Result<(), Error> {
    let pegin_conf_depth = BOTANIX_TESTNET.parent_confirmation_depth;
    it_info_print!("Pegin Confirmation Depth", pegin_conf_depth);
    let bitcoind_rpc = suite.global_context.bitcoind_rpc();

    // Load up the bitcoin wallet and generate some blocks
    for wallet in bitcoind_rpc.list_wallets().unwrap() {
        it_info_print!("#UNLOADING WALLET?", &wallet);
        let _ = bitcoind_rpc.unload_wallet(Some(&wallet));
    }
    let create_res = bitcoind_rpc.create_wallet(BITCOIND_WALLET_NAME, None, None, None, None);
    if create_res.is_err() {
        // wallet already exists, load wallet
        let _ = bitcoind_rpc.load_wallet(BITCOIND_WALLET_NAME);
    }
    let address =
        bitcoind_rpc.get_new_address(None, None).expect("get new address").assume_checked();
    // generate > 100 blocks so coinbase utxos can be spent from the wallet
    bitcoind_rpc.generate_to_address(101, &address).expect("generate to address");
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Print wallet balancee
    let mut balance = bitcoind_rpc.get_balance(None, None).expect("get balance");
    it_info_print!("Wallet Balance", balance);

    let test_fed_members = suite
        .local_context
        .poa_nodes
        .as_ref()
        .expect("test federation member configurations")
        .clone();
    let mut rx = suite.local_context.poa_notification.as_ref().expect("poa notifs").subscribe();

    // generate mint contract test instances
    let mut mint_contract_instances = Vec::new();
    for (index, _) in test_fed_members.iter() {
        let botanix_eth_client =
            test_fed_members.get(index).cloned().unwrap().create_botanix_eth_client().await;
        mint_contract_instances.push(botanix_eth_client);
    }

    // Provider to one of the federation members
    let provider = Provider::<Http>::try_from(format!(
        "http://localhost:{}",
        test_fed_members.get(&0).unwrap().rpc_port
    ))
    .expect("could not instantiate HTTP Provider");

    // Set up dummy eth address
    let mut pegin_txsids = Vec::new();
    let eth_destination = ethers::core::types::Address::random();
    // get gateway address
    let gateway_address_response = provider
        .request::<Vec<String>, GatewayAddressResponse>(
            "eth_getGatewayAddress",
            vec![hex::encode(eth_destination.0)],
        )
        .await
        .expect("should get gateway address");
    let btc_address = bitcoin::Address::from_str(gateway_address_response.gateway_address.as_str())
        .expect("valid btc_address")
        .assume_checked();
    for index in 0..NUM_PEGINS - 1 {
        it_info_print!("Pegin #", index);
        it_info_print!("Pegin amount", Amount::from_sat(balance.to_sat() / NUM_PEGINS as u64));

        let pegin_txid = bitcoind_rpc
            .send_to_address(
                &btc_address,
                Amount::from_sat(balance.to_sat() / NUM_PEGINS as u64),
                None,
                None,
                Some(true),
                None,
                Some(1),
                None,
            )
            .expect("valid send");
        let agg_pk =
            secp256k1::PublicKey::from_str(gateway_address_response.aggregate_public_key.as_str())
                .expect("valid agg pk");
        let blocks = bitcoind_rpc.generate_to_address(1, &address).expect("generate to address");

        pegin_txsids.push((
            pegin_txid,
            eth_destination,
            btc_address.clone(),
            agg_pk,
            blocks[0],
            vec![],
        ));
    }

    // Generate some block to confirm all pegins
    bitcoind_rpc
        .generate_to_address(1 + pegin_conf_depth as u64, &address)
        .expect("generate to address");
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Lets assemble the headers we need for the proof
    // We will collect all the headers all the way up to the tip which is not needed, but allowed.
    // In theory, we only need to collect headers from the block our pegin is in, to the finalized
    // block (the one in the mainchain commitment).
    let tip = bitcoind_rpc.get_best_block_hash().unwrap();
    it_info_print!("Bitcoin Chain Tip", tip);
    let tip_header = bitcoind_rpc.get_block_header(&tip).expect("valid block header");

    for index in 0..NUM_PEGINS - 1 {
        let i = index as usize;
        // Get confirmation block hash for each pegin and build headers list
        let conf_hash = &pegin_txsids[i].4;
        let mut headers = vec![];
        let mut cursor = tip_header;
        let mut stopgap = 2000; // just to make sure we don't infinite loop until genesis
        loop {
            stopgap -= 1;
            if stopgap == 0 || cursor.prev_blockhash == bitcoin::BlockHash::all_zeros() {
                panic!("confirmation block not found...");
            }

            headers.push(cursor);
            if cursor.block_hash() == *conf_hash {
                break;
            }
            cursor = bitcoind_rpc.get_block_header(&cursor.prev_blockhash).unwrap();
        }
        headers.reverse();

        // add headers to pegin
        pegin_txsids[i].5 = headers.clone();
        it_info_print!("Pegin #", index);
        it_info_print!("Number of pegin_headers:", headers.len());
    }

    let checkpoint = {
        let tip = bitcoind_rpc.get_block_count().unwrap();
        let height = tip - pegin_conf_depth as u64;
        let hash = bitcoind_rpc.get_block_hash(height).unwrap();
        (bitcoind_rpc.get_block_header(&hash).unwrap(), height as u32)
    };

    let mut pegins = vec![];
    for (txid, eth_address, btc_address, agg_pk, _block, headers) in pegin_txsids {
        // retrieve the transaction
        let tx_res = bitcoind_rpc.get_transaction(&txid, None).expect("valid tx");
        assert!(tx_res.info.confirmations > 1);
        let pegin_tx = tx_res.transaction().expect("valid tx");
        let eth_account = Address::from_slice(eth_address.as_slice());
        let (vout, pegin_output) = pegin_tx
            .output
            .iter()
            .enumerate()
            .find(|(_, o)| o.script_pubkey == btc_address.script_pubkey())
            .unwrap();
        let amount = pegin_output.value.to_wei();
        it_info_print!("Btc Amount", amount);

        // first we need the block hash of the block with the conf'd pegin tx
        let conf_hash = tx_res.info.blockhash.expect("pegin confirmed");
        let conf_block_info = bitcoind_rpc.get_block_info(&conf_hash).expect("valid txids");
        it_info_print!("Block info", conf_block_info);
        let txids = conf_block_info.tx;
        it_info_print!("Txids", txids);
        assert!(txids.contains(&txid), "block should contain pegin tx");
        let matches = txids.iter().map(|t| t == &txid).collect::<Vec<_>>();
        it_info_print!("Matches", matches);
        let pmt = PartialMerkleTree::from_txids(&txids, &matches);

        // create pegin meta
        let bitcoin_block_height = conf_block_info.height;
        let meta = PeginMeta {
            version: 0,
            outpoint: bitcoin::OutPoint::new(pegin_tx.txid(), vout as u32),
            address: eth_account,
            aggregate_publickey: agg_pk,
            tx: pegin_tx.clone(),
            merkle_proof: pmt,
            block_headers: headers.clone(),
        };

        // validate the pegin data first offchain before submitting
        let pegin_data = PeginData {
            account: Address::from_slice(eth_address.as_bytes()),
            amount,
            bitcoin_block_height: bitcoin_block_height as u32,
            meta: vec![meta.clone()],
        };

        pegin_data.validate(&checkpoint, &agg_pk).expect("pegin data should be valid!");
        pegins.push(pegin_data);
    }

    // mint all the pegins across multiple blocks
    let refund_address = ethers::core::types::Address::random();
    let mut tx_hashes = vec![];
    let provider = test_fed_members.get(&0).unwrap().create_botanix_eth_client().await;
    let mut nonce = provider.nonce().await;
    for (index, pegin) in pegins.iter().enumerate() {
        it_info_print!("Pegin #", index);

        // There is only one pegin that needs to be serialized
        let serialized_pegin_meta = pegin.meta[0].serialize();
        let metadata = ethers::core::types::Bytes::from(serialized_pegin_meta.clone());

        // check of block if full (max gas limit = 30 million)
        let block = provider.get_pending_block().await.expect("block to exist");
        // if gas used > 28 million, then we need to wait for the next block
        it_info_print!("Block Gas Used", block.gas_used);
        if block.gas_used.as_u64() > 28_000_000_u64 {
            it_info_print!("Block is full, waiting for next block");

            it_info_print!("Waiting for current Pegins to be mined!");
            let http_provider = provider.provider().clone();
            for (index, tx_hash) in tx_hashes.clone().iter().enumerate() {
                let pending_tx = PendingTransaction::new(
                    ethers::core::types::H256::from(tx_hash),
                    &http_provider,
                );

                pending_tx.await.expect("tx should be mined");
                it_info_print!("Pegin mined!", tx_hash);

                // remove the tx from the list
                tx_hashes.remove(index);
            }
        }

        let tx_hash = provider
            .non_confirmed_mint(
                ethers::core::types::Address::from_slice(pegin.account.as_slice()),
                pegin.amount,
                pegin.bitcoin_block_height,
                metadata,
                refund_address,
                nonce,
            )
            .await
            .unwrap();

        nonce += ethers::core::types::U256::one();
        tx_hashes.push(tx_hash);
    }

    it_info_print!("Minted all the pegins");
    tokio::time::sleep(Duration::from_secs(5)).await;
    // Ensure eth address has a non zero balance
    let eth_address_balance = provider
        .get_botanix_balance(eth_destination.to_string().as_str())
        .await
        .expect("balance to exist");
    assert!(!eth_address_balance.is_zero());

    // Check refund address has a non zero balance
    let refund_address_balance = provider.get_balance(refund_address).await;
    assert!(!refund_address_balance.expect("get balance").is_zero());

    // send a pegout which should be successful bc inputs <= MAX_EDH_SIZE
    // Generate and send pegout tx: arbitrarily choose the first btc address
    let pegout_destination =
        ethers::core::types::Bytes::from(address.to_string().as_bytes().to_vec());
    // use empty pegout data
    let pegout_data = ethers::core::types::Bytes::new();
    let pegout_amount = eth_address_balance;
    let pegout_tx_receipt =
        provider.burn(pegout_destination, pegout_data, pegout_amount).await.unwrap();
    it_info_print!("Pegout Tx Receipt: ", pegout_tx_receipt);

    // wait for the tx to be included in a botanix block
    await_botanix_event(&mut rx, *BURN_TOPIC).await;

    // make sure we have enough time for the nonce to be updated
    tokio::time::sleep(Duration::from_secs(20)).await;

    // need two txs to enter an epoch
    let eoa_tx_receipt =
        provider.send_eoa(ethers::core::types::Address::random(), SEND_AMOUNT).await.unwrap();
    it_info_print!("Eoa Tx Receipt: ", eoa_tx_receipt);

    // sleep for a few more seconds
    tokio::time::sleep(Duration::from_secs(20)).await;

    let eoa_tx_receipt =
        provider.send_eoa(ethers::core::types::Address::random(), SEND_AMOUNT).await.unwrap();
    it_info_print!("Eoa Tx Receipt: ", eoa_tx_receipt);

    // sleep for a few more seconds
    tokio::time::sleep(Duration::from_secs(20)).await;

    // Retrieve the last block
    let tip_hash = bitcoind_rpc.get_best_block_hash().expect("valid block hash");
    let tip_block = bitcoind_rpc.get_block(&tip_hash).expect("valid block");
    // there should be 2 transaction one of which is the pegout the other is coinbase
    it_info_print!("txData: ", tip_block.txdata);
    assert_eq!(tip_block.txdata.len(), 2);
    let pegout_tx = tip_block.txdata.get(1).unwrap();
    it_info_print!("Pegout tx: ", pegout_tx);

    Ok(())
}
