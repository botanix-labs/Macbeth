use crate::suite::consensus::common::events::SEND_AMOUNT;
use std::{str::FromStr, time::Duration};

use bitcoin::{
    blockdata::block::Header, hashes::Hash, merkle_tree::PartialMerkleTree, Amount, Txid,
};
use bitcoincore_rpc::RpcApi;
use ethers::{prelude::Provider, providers::Http};
use reth_primitives::{
    botanix::{
        peg_contract::{
            PeginMeta, PeginMetaV0, PeginMetaV1, PEGIN_META_VERSION_V0, PEGIN_META_VERSION_V1,
        },
        utils::AmountExt,
    },
    revm_primitives::{Address, FixedBytes, B256},
};
use serde_json::json;

use crate::{
    it_info_print,
    suite::consensus::{
        common::events::{BlockWithEDH, GatewayAddressResponse},
        ConsensusIntegrationTestSuite,
    },
    utils::generate_blocks,
};

// Helper function to generate invalid pegin metas
async fn generate_invalid_pegin_metas(
    pegin_tx: bitcoin::Transaction,
    vout: u32,
    gateway_address_response: GatewayAddressResponse,
    eth_account: Address,
    pmt: PartialMerkleTree,
    headers: Vec<Header>,
    provider: Provider<Http>,
) -> Vec<(Vec<PeginMeta>, &'static str)> {
    let mut invalid_pegin_meta_cases = Vec::new();

    // Create invalid pegin meta with empty headers list
    let empty_headers_meta = vec![PeginMeta::V0(PeginMetaV0 {
        version: PEGIN_META_VERSION_V0,
        outpoint: bitcoin::OutPoint::new(pegin_tx.compute_txid(), vout),
        address: eth_account.clone(),
        aggregate_publickey: secp256k1::PublicKey::from_str(
            gateway_address_response.aggregate_public_key.as_str(),
        )
        .expect("valid public key"),
        tx: pegin_tx.clone(),
        merkle_proof: pmt.clone(),
        block_headers: vec![],
    })];
    invalid_pegin_meta_cases.push((empty_headers_meta, "Empty headers list"));

    // Create invalid pegin meta with invalid merkle proof
    let invalid_pmt = PartialMerkleTree::from_txids(&[Txid::all_zeros()], &[true]);
    let invalid_pmt_meta = vec![PeginMeta::V0(PeginMetaV0 {
        version: PEGIN_META_VERSION_V0,
        outpoint: bitcoin::OutPoint::new(pegin_tx.compute_txid(), vout),
        address: eth_account.clone(),
        aggregate_publickey: secp256k1::PublicKey::from_str(
            gateway_address_response.aggregate_public_key.as_str(),
        )
        .expect("valid public key"),
        tx: pegin_tx.clone(),
        merkle_proof: invalid_pmt,
        block_headers: headers.clone(),
    })];
    invalid_pegin_meta_cases.push((invalid_pmt_meta, "Invalid merkle proof"));

    // Create invalid pegin meta with invalid reference hash for v1
    let invalid_ref_hash_meta = vec![PeginMeta::V1(PeginMetaV1 {
        inner: PeginMetaV0 {
            version: PEGIN_META_VERSION_V1,
            outpoint: bitcoin::OutPoint::new(pegin_tx.compute_txid(), vout),
            address: eth_account.clone(),
            aggregate_publickey: secp256k1::PublicKey::from_str(
                gateway_address_response.aggregate_public_key.as_str(),
            )
            .expect("valid public key"),
            tx: pegin_tx.clone(),
            merkle_proof: pmt.clone(),
            block_headers: headers.clone(),
        },
        ref_block_hash: B256::from_slice(&[0; 32]),
    })];
    invalid_pegin_meta_cases.push((invalid_ref_hash_meta, "Invalid reference hash for v1"));

    // Create invalid pegin meta v1 with incorrect version
    let latest_block_with_edh = provider
        .request::<Vec<serde_json::Value>, BlockWithEDH>(
            "eth_getBlockByNumber",
            vec![json!("latest"), json!(false), json!(true)],
        )
        .await;
    let latest_block_with_edh = latest_block_with_edh.expect("valid block with edh");
    let latest_block_hash = latest_block_with_edh.hash;
    let ref_block_hash =
        FixedBytes::<32>::from_str(&latest_block_hash.as_str()).expect("valid hash");
    let meta_v1_with_incorrect_version = vec![PeginMeta::V1(PeginMetaV1 {
        inner: PeginMetaV0 {
            version: PEGIN_META_VERSION_V0,
            outpoint: bitcoin::OutPoint::new(pegin_tx.compute_txid(), vout),
            address: eth_account.clone(),
            aggregate_publickey: secp256k1::PublicKey::from_str(
                gateway_address_response.aggregate_public_key.as_str(),
            )
            .expect("valid public key"),
            tx: pegin_tx.clone(),
            merkle_proof: pmt.clone(),
            block_headers: headers.clone(),
        },
        ref_block_hash,
    })];
    invalid_pegin_meta_cases
        .push((meta_v1_with_incorrect_version, "V1 with incorrect version (V0)"));

    // Create invalid pegin meta v0 with incorrect version
    let meta_v0_with_incorrect_version = vec![PeginMeta::V0(PeginMetaV0 {
        version: PEGIN_META_VERSION_V1,
        outpoint: bitcoin::OutPoint::new(pegin_tx.compute_txid(), vout),
        address: eth_account.clone(),
        aggregate_publickey: secp256k1::PublicKey::from_str(
            gateway_address_response.aggregate_public_key.as_str(),
        )
        .expect("valid public key"),
        tx: pegin_tx.clone(),
        merkle_proof: pmt.clone(),
        block_headers: headers.clone(),
    })];
    invalid_pegin_meta_cases
        .push((meta_v0_with_incorrect_version, "V0 with incorrect version (V1)"));

    // Create invalid pegin meta with proofs having mixed versions
    let mixed_versions_meta = vec![
        PeginMeta::V0(PeginMetaV0 {
            version: PEGIN_META_VERSION_V0,
            outpoint: bitcoin::OutPoint::new(pegin_tx.compute_txid(), vout),
            address: eth_account.clone(),
            aggregate_publickey: secp256k1::PublicKey::from_str(
                gateway_address_response.aggregate_public_key.as_str(),
            )
            .expect("valid public key"),
            tx: pegin_tx.clone(),
            merkle_proof: pmt.clone(),
            block_headers: headers.clone(),
        }),
        PeginMeta::V1(PeginMetaV1 {
            inner: PeginMetaV0 {
                version: PEGIN_META_VERSION_V1,
                outpoint: bitcoin::OutPoint::new(pegin_tx.compute_txid(), vout),
                address: eth_account.clone(),
                aggregate_publickey: secp256k1::PublicKey::from_str(
                    gateway_address_response.aggregate_public_key.as_str(),
                )
                .expect("valid public key"),
                tx: pegin_tx.clone(),
                merkle_proof: pmt.clone(),
                block_headers: headers.clone(),
            },
            ref_block_hash,
        }),
    ];
    invalid_pegin_meta_cases
        .push((mixed_versions_meta, "Mixed versions (V0 and V1) in same vector"));

    // Create invalid pegin meta with v1 proofs having mixed ref block hashes
    let first_block_with_edh = provider
        .request::<Vec<serde_json::Value>, BlockWithEDH>(
            "eth_getBlockByNumber",
            vec![json!("earliest"), json!(false), json!(true)],
        )
        .await;
    let first_block_with_edh = first_block_with_edh.expect("valid block with edh");
    let first_block_hash = first_block_with_edh.hash;
    let ref_block_hash_first =
        FixedBytes::<32>::from_str(&first_block_hash.as_str()).expect("valid hash");
    let mixed_ref_block_hashes_meta = vec![
        PeginMeta::V1(PeginMetaV1 {
            inner: PeginMetaV0 {
                version: PEGIN_META_VERSION_V1,
                outpoint: bitcoin::OutPoint::new(pegin_tx.compute_txid(), vout),
                address: eth_account.clone(),
                aggregate_publickey: secp256k1::PublicKey::from_str(
                    gateway_address_response.aggregate_public_key.as_str(),
                )
                .expect("valid public key"),
                tx: pegin_tx.clone(),
                merkle_proof: pmt.clone(),
                block_headers: headers.clone(),
            },
            ref_block_hash,
        }),
        PeginMeta::V1(PeginMetaV1 {
            inner: PeginMetaV0 {
                version: PEGIN_META_VERSION_V1,
                outpoint: bitcoin::OutPoint::new(pegin_tx.compute_txid(), vout),
                address: eth_account.clone(),
                aggregate_publickey: secp256k1::PublicKey::from_str(
                    gateway_address_response.aggregate_public_key.as_str(),
                )
                .expect("valid public key"),
                tx: pegin_tx.clone(),
                merkle_proof: pmt.clone(),
                block_headers: headers.clone(),
            },
            ref_block_hash: ref_block_hash_first,
        }),
    ];
    invalid_pegin_meta_cases
        .push((mixed_ref_block_hashes_meta, "V1 proofs with mismatched reference block hashes"));

    invalid_pegin_meta_cases
}

#[allow(clippy::too_many_lines)]
pub async fn invalid_pegin(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), super::error::InvalidTransactionError> {
    let pegin_conf_depth = 6; //TODO(stevenroose) set this from chain constant?

    // Set up regtest connection
    // config is hardcoded to only work with regtest
    let bitcoind_rpc = suite.global_context.bitcoind_rpc();
    tokio::time::sleep(Duration::from_secs(5)).await;

    let test_fed_members = suite
        .local_context
        .poa_nodes
        .as_ref()
        .expect("test federation member configurations")
        .clone();

    // subscribe to notifications so channel stays open
    let _rx = suite.local_context.poa_notification.as_ref().expect("poa notifs").subscribe();

    // generate mint contract test instances
    let mut mint_contract_instances = Vec::new();
    for (index, _) in test_fed_members.iter() {
        let botanix_eth_client =
            test_fed_members.get(index).cloned().unwrap().botanix_eth_client.clone();
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
    generate_blocks(&bitcoind_rpc, 2 + pegin_conf_depth).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // retrieve the transaction
    let tx_res = bitcoind_rpc.get_transaction(&pegin_txid, None).expect("valid tx");
    assert!(tx_res.info.confirmations > 1);
    let pegin_tx = tx_res.transaction().expect("valid tx");
    it_info_print!("Bitcoin pegin Tx", pegin_tx);
    it_info_print!("Gateway Data", gateway_address_response);
    it_info_print!("Gateway Data Pub key", gateway_address_response.aggregate_public_key);

    let eth_account = Address::from_slice(eth_destination.as_bytes());
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

    // create partial merkle tree
    let conf_hash = tx_res.info.blockhash.expect("pegin confirmed");
    let conf_block_info = bitcoind_rpc.get_block_info(&conf_hash).expect("valid txids");
    it_info_print!("Block info", conf_block_info);
    let pmt = PartialMerkleTree::from_txids(&conf_block_info.tx, &[false, true]);

    let bitcoin_block_height = conf_block_info.height;

    // Generate invalid pegin metas
    let invalid_pegin_metas = generate_invalid_pegin_metas(
        pegin_tx,
        vout as u32,
        gateway_address_response,
        eth_account,
        pmt,
        headers,
        provider,
    )
    .await;

    let mut botanix_eth_client = mint_contract_instances
        .first()
        .cloned()
        .unwrap()
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

    for (invalid_pegin_meta, description) in invalid_pegin_metas {
        it_info_print!("Invalid pegin meta: {}", description);
        let mut serialized_pegin_meta = Vec::new();
        for meta in invalid_pegin_meta {
            let serialized = meta.serialize().unwrap();
            serialized_pegin_meta.extend_from_slice(&serialized);
        }
        it_info_print!("Serialized pegin meta: ", hex::encode(serialized_pegin_meta.clone()));

        // send the pegin transactions to all fed members
        let metadata = ethers::core::types::Bytes::from(serialized_pegin_meta.clone());

        // pegin address balance before pegin
        let eth_pegin_address = eth_account.to_string();
        it_info_print!("Pegin address", eth_pegin_address);
        let addr =
            reth_primitives::Address::from_str(&eth_pegin_address).expect("valid eth address");
        let pegin_address_initial_balance =
            botanix_eth_client.get_botanix_balance(addr).await.unwrap();
        it_info_print!("Initial pegin address balance", pegin_address_initial_balance);

        // nonce before pegin
        let sender_address = botanix_eth_client.get_sender_address();
        it_info_print!("Sender address", sender_address);
        let nonce_before = botanix_eth_client.get_nonce(sender_address.clone()).await.unwrap();
        it_info_print!("Nonce before pegin", nonce_before);

        // attempt to mint the invalid pegin
        // call mint attack contract so we test internal calls to Minting.sol
        // and not just top level (EOA) calls
        let tx_receipt = botanix_eth_client
            .mint_attack(
                eth_destination.clone(),
                amount,
                bitcoin_block_height as u32,
                metadata,
                ethers::core::types::Address::random(),
            )
            .await
            .unwrap()
            .unwrap();

        // status should be 0 (failure)
        it_info_print!("Pegin Tx Receipt", tx_receipt);
        assert!(tx_receipt.status.unwrap().is_zero());

        // pegin address balance after pegin
        let pegin_address_final_balance =
            botanix_eth_client.get_botanix_balance(addr).await.unwrap();
        it_info_print!("Final pegin address balance", pegin_address_final_balance);

        assert_eq!(pegin_address_initial_balance, pegin_address_final_balance);

        // nonce after pegin
        let nonce_after = botanix_eth_client.get_nonce(sender_address).await.unwrap();
        it_info_print!("Nonce after pegin", nonce_after);

        assert!(nonce_after > nonce_before);
    }

    Ok(())
}
