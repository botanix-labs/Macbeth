use crate::{
    suite::consensus::common::events::SEND_AMOUNT,
    utils::{generate_blocks, MIN_BLOCKS_COINBASE_MATURE},
};
use std::{str::FromStr, time::Duration};

use bitcoin::{
    blockdata::block::Header, hashes::Hash, merkle_tree::PartialMerkleTree, Amount, Txid,
};
use bitcoincore_rpc::RpcApi;
use ethers::{prelude::Provider, providers::Http};
use reth_primitives::{
    botanix::{
        peg_contract::{PeginMeta, PeginMetaV0, PEGIN_META_VERSION_V0},
        utils::AmountExt,
    },
    Address,
};

use crate::{
    it_info_print,
    suite::consensus::{common::events::GatewayAddressResponse, ConsensusIntegrationTestSuite},
};

#[allow(clippy::too_many_lines)]
pub async fn multi_pegin_revert_scenarios(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), super::error::InvalidTransactionError> {
    let pegin_conf_depth = reth_chainspec::BOTANIX_TESTNET.parent_confirmation_depth;

    // Set up regtest connection
    let bitcoind_rpc = suite.global_context.bitcoind_rpc();
    tokio::time::sleep(Duration::from_secs(5)).await;

    let test_fed_members = suite
        .local_context
        .poa_nodes
        .as_ref()
        .expect("test federation member configurations")
        .clone();

    // Use the client from the first PoA node
    let mut botanix_eth_client = test_fed_members
        .get(&0)
        .cloned()
        .unwrap()
        .botanix_eth_client
        .clone()
        .expect("Botanix Client must be initialized");

    let provider = Provider::<Http>::try_from(format!(
        "http://localhost:{}",
        test_fed_members.get(&0).unwrap().rpc_port
    ))
    .expect("could not instantiate HTTP Provider");

    // Set up three distinct dummy eth addresses
    let eth_destination1 = ethers::core::types::Address::random();
    let eth_destination2 = ethers::core::types::Address::random();
    let eth_destination3 = ethers::core::types::Address::random();
    let eth_account1 = Address::from_slice(eth_destination1.as_bytes());
    let eth_account2 = Address::from_slice(eth_destination2.as_bytes());
    let eth_account3 = Address::from_slice(eth_destination3.as_bytes());

    // get gateway address
    let gateway_address_response = provider
        .request::<Vec<String>, GatewayAddressResponse>(
            "eth_getGatewayAddress",
            vec![hex::encode(eth_destination1.0)],
        )
        .await
        .expect("should get gateway address");

    it_info_print!("Gateway Address Response", gateway_address_response);
    let btc_address = bitcoin::Address::from_str(gateway_address_response.gateway_address.as_str())
        .expect("valid btc_address")
        .assume_checked();

    // Make sure we have mature coins
    let balance = bitcoind_rpc.get_balance(None, Some(true)).expect("get balance");
    if balance < Amount::from_btc(5.0).unwrap() {
        // Need ~4.5 BTC for pegins + fees
        it_info_print!("Generating initial blocks for mature coins...");
        generate_blocks(&bitcoind_rpc, MIN_BLOCKS_COINBASE_MATURE).await;
    }
    it_info_print!("Bitcoin balance", bitcoind_rpc.get_balance(None, None).unwrap());

    // --- Setup Pegins (1, 1.5, 2 BTC) ---
    let amount_btc1 = Amount::ONE_BTC;
    let pegin_txid1 = bitcoind_rpc
        .send_to_address(&btc_address, amount_btc1, None, None, Some(true), None, Some(1), None)
        .expect("valid send 1");
    let amount_btc2 = Amount::from_btc(1.5).unwrap();
    let pegin_txid2 = bitcoind_rpc
        .send_to_address(&btc_address, amount_btc2, None, None, Some(true), None, Some(1), None)
        .expect("valid send 2");
    let amount_btc3 = Amount::from_btc(2.0).unwrap();
    let pegin_txid3 = bitcoind_rpc
        .send_to_address(&btc_address, amount_btc3, None, None, Some(true), None, Some(1), None)
        .expect("valid send 3");
    it_info_print!("Sent Pegin Tx 1 (1 BTC)", pegin_txid1);
    it_info_print!("Sent Pegin Tx 2 (1.5 BTC)", pegin_txid2);
    it_info_print!("Sent Pegin Tx 3 (2 BTC)", pegin_txid3);

    // Generate enough blocks to confirm all transactions
    let mined_in_block = generate_blocks(&bitcoind_rpc, 1).await; // Try to mine in same block
    it_info_print!("Mined pegins in block(s)", mined_in_block);
    generate_blocks(&bitcoind_rpc, pegin_conf_depth).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Retrieve data for Pegins
    let tx_res1 = bitcoind_rpc.get_transaction(&pegin_txid1, None).expect("valid tx 1");
    let pegin_tx1 = tx_res1.transaction().expect("valid tx 1");
    let (vout1, _) = pegin_tx1
        .output
        .iter()
        .enumerate()
        .find(|(_, o)| o.script_pubkey == btc_address.script_pubkey())
        .unwrap();
    let conf_hash1 = tx_res1.info.blockhash.expect("pegin 1 confirmed");

    let tx_res2 = bitcoind_rpc.get_transaction(&pegin_txid2, None).expect("valid tx 2");
    let pegin_tx2 = tx_res2.transaction().expect("valid tx 2");
    let (vout2, _) = pegin_tx2
        .output
        .iter()
        .enumerate()
        .find(|(_, o)| o.script_pubkey == btc_address.script_pubkey())
        .unwrap();
    let conf_hash2 = tx_res2.info.blockhash.expect("pegin 2 confirmed");

    let tx_res3 = bitcoind_rpc.get_transaction(&pegin_txid3, None).expect("valid tx 3");
    let pegin_tx3 = tx_res3.transaction().expect("valid tx 3");
    let (vout3, pegin_output3) = pegin_tx3
        .output
        .iter()
        .enumerate()
        .find(|(_, o)| o.script_pubkey == btc_address.script_pubkey())
        .unwrap();
    let amount_wei3 = pegin_output3.value.to_wei(); // Only need amount for the valid pegin
    let conf_hash3 = tx_res3.info.blockhash.expect("pegin 3 confirmed");

    // Ensure all confirmed in the same block for simplicity
    assert_eq!(conf_hash1, conf_hash2, "Pegin 1 & 2 must be confirmed in the same block");
    assert_eq!(conf_hash2, conf_hash3, "Pegin 2 & 3 must be confirmed in the same block");
    let conf_hash = conf_hash1;
    it_info_print!("All Pegins Confirmed in block", conf_hash);

    // --- Common Proof Generation ---
    let tip = bitcoind_rpc.get_best_block_hash().unwrap();
    let tip_header = bitcoind_rpc.get_block_header(&tip).expect("valid block header");
    let mut headers = vec![];
    let mut cursor = tip_header;
    let mut stopgap = 200;
    loop {
        if stopgap == 0 || cursor.block_hash() == bitcoin::BlockHash::all_zeros() {
            panic!("conf block not found");
        }
        stopgap -= 1;
        headers.push(cursor);
        if cursor.block_hash() == conf_hash {
            break;
        }
        cursor = bitcoind_rpc.get_block_header(&cursor.prev_blockhash).unwrap();
    }
    headers.reverse();
    it_info_print!("Number of pegin headers: {}", headers.len());

    let conf_block_info = bitcoind_rpc.get_block_info(&conf_hash).expect("valid block info");
    let bitcoin_block_height = conf_block_info.height as u32;
    let num_txs = conf_block_info.tx.len();

    // Create PMT for Pegin 1
    let index1 = conf_block_info.tx.iter().position(|id| id == &pegin_txid1).unwrap();
    let mut flags1 = vec![false; num_txs];
    flags1[index1] = true;
    let _pmt1 = PartialMerkleTree::from_txids(&conf_block_info.tx, &flags1);

    // Create PMT for Pegin 2
    let index2 = conf_block_info.tx.iter().position(|id| id == &pegin_txid2).unwrap();
    let mut flags2 = vec![false; num_txs];
    flags2[index2] = true;
    let pmt2 = PartialMerkleTree::from_txids(&conf_block_info.tx, &flags2);

    // Create PMT for Pegin 3
    let index3 = conf_block_info.tx.iter().position(|id| id == &pegin_txid3).unwrap();
    let mut flags3 = vec![false; num_txs];
    flags3[index3] = true;
    let pmt3 = PartialMerkleTree::from_txids(&conf_block_info.tx, &flags3);

    // --- Setup Aggregate Public Key ---
    let agg_pk =
        secp256k1::PublicKey::from_str(gateway_address_response.aggregate_public_key.as_str())
            .expect("valid public key");

    // --- Deploy Helper Contract ---
    let contract_deployer =
        botanix_eth_client.get_contract_deployer().expect("To get contract deployer");
    let _tx_receipt = botanix_eth_client
        .send_eoa(contract_deployer.address(), SEND_AMOUNT)
        .await
        .expect("To send eoa")
        .expect("To get tx receipt");
    it_info_print!("Deploying MultiMintHelper contract...");
    let helper_contract_address = botanix_eth_client
        .deploy_multi_mint_helper_contract(contract_deployer)
        .await
        .expect("To deploy multi mint helper contract");
    botanix_eth_client.set_multi_mint_helper_contract(helper_contract_address);
    it_info_print!("MultiMintHelper contract deployed at", helper_contract_address);
    let mint_contract_address = botanix_eth_client.mint_contract.address();
    let mint_contract_initial_balance = botanix_eth_client
        .get_botanix_balance(Address::from(mint_contract_address.0))
        .await
        .unwrap();
    it_info_print!("Mint contract initial balance", mint_contract_initial_balance);

    // ==========================================
    // === Scenario 1: Invalid + Invalid Pegin ===
    // ==========================================
    it_info_print!("Starting Scenario 1: Invalid Pegin + Invalid Pegin");

    // Invalid Meta 1: Invalid Merkle Proof (using Pegin 1 data)
    let invalid_pmt = PartialMerkleTree::from_txids(&[Txid::all_zeros()], &[true]);
    let invalid_pegin_meta1 = PeginMeta::V0(PeginMetaV0 {
        version: PEGIN_META_VERSION_V0,
        outpoint: bitcoin::OutPoint::new(pegin_tx1.compute_txid(), vout1 as u32),
        address: eth_account1, // Destination doesn't matter much as it should revert
        aggregate_publickey: agg_pk.clone(),
        tx: pegin_tx1.clone(),
        merkle_proof: invalid_pmt.clone(), // Invalid proof - Clone here
        block_headers: headers.clone(),
    });
    let serialized_invalid_meta1 = invalid_pegin_meta1.serialize().unwrap();

    // Invalid Meta 2: Empty Headers List (using Pegin 2 data)
    let invalid_pegin_meta2 = PeginMeta::V0(PeginMetaV0 {
        version: PEGIN_META_VERSION_V0,
        outpoint: bitcoin::OutPoint::new(pegin_tx2.compute_txid(), vout2 as u32),
        address: eth_account2, // Destination doesn't matter much
        aggregate_publickey: agg_pk.clone(),
        tx: pegin_tx2.clone(),
        merkle_proof: pmt2.clone(), // Valid proof here
        block_headers: vec![],      // Invalid headers
    });
    let serialized_invalid_meta2 = invalid_pegin_meta2.serialize().unwrap();

    // Get initial balances & block heights for Scenario 1
    let balance1_before_s1 = botanix_eth_client.get_balance(eth_destination1).await.unwrap();
    let balance2_before_s1 = botanix_eth_client.get_balance(eth_destination2).await.unwrap();
    let height1_before_s1 = botanix_eth_client
        .mint_contract
        .pegin_bitcoin_block_height(eth_destination1)
        .await
        .unwrap();
    let height2_before_s1 = botanix_eth_client
        .mint_contract
        .pegin_bitcoin_block_height(eth_destination2)
        .await
        .unwrap();

    it_info_print!("Calling multiMintTwo with two invalid pegins...");
    let tx_receipt_s1 = botanix_eth_client
        .multi_mint_two(
            eth_destination1,
            1_000_000_000_000_000_000u64.into(), // Amount doesn't strictly matter for revert
            bitcoin_block_height,
            ethers::core::types::Bytes::from(serialized_invalid_meta1),
            ethers::core::types::Address::random(),
            eth_destination2,
            1_500_000_000_000_000_000u64.into(), // Amount doesn't strictly matter for revert
            bitcoin_block_height,
            ethers::core::types::Bytes::from(serialized_invalid_meta2),
            ethers::core::types::Address::random(),
        )
        .await
        .expect("multi_mint_two call (invalid+invalid) should execute")
        .expect("Failed to get receipt for multi_mint_two call (invalid+invalid)");

    it_info_print!("Scenario 1 Tx Receipt", tx_receipt_s1);
    assert_eq!(tx_receipt_s1.status.unwrap().as_u64(), 0, "Scenario 1 TX should revert");

    // Check balances & block heights unchanged
    let balance1_after_s1 = botanix_eth_client.get_balance(eth_destination1).await.unwrap();
    let balance2_after_s1 = botanix_eth_client.get_balance(eth_destination2).await.unwrap();
    let height1_after_s1 = botanix_eth_client
        .mint_contract
        .pegin_bitcoin_block_height(eth_destination1)
        .await
        .unwrap();
    let height2_after_s1 = botanix_eth_client
        .mint_contract
        .pegin_bitcoin_block_height(eth_destination2)
        .await
        .unwrap();
    let mint_contract_balance_after_s1 = botanix_eth_client
        .get_botanix_balance(Address::from(mint_contract_address.0))
        .await
        .unwrap();
    assert_eq!(balance1_before_s1, balance1_after_s1, "Scenario 1 Balance 1 should be unchanged");
    assert_eq!(balance2_before_s1, balance2_after_s1, "Scenario 1 Balance 2 should be unchanged");
    assert_eq!(height1_before_s1, height1_after_s1, "Scenario 1 Height 1 should be unchanged");
    assert_eq!(height2_before_s1, height2_after_s1, "Scenario 1 Height 2 should be unchanged");
    assert_eq!(
        mint_contract_balance_after_s1, mint_contract_initial_balance,
        "Mint contract balance should be unchanged"
    );
    it_info_print!(
        "Scenario 1 Verified: Transaction reverted, balances and block heights unchanged."
    );

    // ==========================================
    // === Scenario 2: Valid + Invalid Pegin ===
    // ==========================================
    it_info_print!("Starting Scenario 2: Valid Pegin + Invalid Pegin");

    // Valid Meta (using Pegin 3 data)
    let valid_pegin_meta = PeginMeta::V0(PeginMetaV0 {
        version: PEGIN_META_VERSION_V0,
        outpoint: bitcoin::OutPoint::new(pegin_tx3.compute_txid(), vout3 as u32),
        address: eth_account3, // Use destination 3 for the valid one
        aggregate_publickey: agg_pk.clone(),
        tx: pegin_tx3.clone(),
        merkle_proof: pmt3.clone(),
        block_headers: headers.clone(),
    });
    let serialized_valid_meta = valid_pegin_meta.serialize().unwrap();

    // Invalid Meta (using Pegin 1 data, but invalid proof again)
    let invalid_pegin_meta_s2 = PeginMeta::V0(PeginMetaV0 {
        version: PEGIN_META_VERSION_V0,
        outpoint: bitcoin::OutPoint::new(pegin_tx1.compute_txid(), vout1 as u32),
        address: eth_account1, // Destination 1
        aggregate_publickey: agg_pk.clone(),
        tx: pegin_tx1.clone(),
        merkle_proof: invalid_pmt, // Use the original value here (no clone needed)
        block_headers: headers.clone(),
    });
    let serialized_invalid_meta_s2 = invalid_pegin_meta_s2.serialize().unwrap();

    // Get initial balances & block heights for Scenario 2
    let balance3_before_s2 = botanix_eth_client.get_balance(eth_destination3).await.unwrap();
    let balance1_before_s2 = botanix_eth_client.get_balance(eth_destination1).await.unwrap();
    let height3_before_s2 = botanix_eth_client
        .mint_contract
        .pegin_bitcoin_block_height(eth_destination3)
        .await
        .unwrap();
    let height1_before_s2 = botanix_eth_client
        .mint_contract
        .pegin_bitcoin_block_height(eth_destination1)
        .await
        .unwrap();

    it_info_print!("Calling multiMintTwo with valid then invalid pegin...");
    let tx_receipt_s2 = botanix_eth_client
        .multi_mint_two(
            eth_destination3, // Valid Pegin (Dest 3)
            amount_wei3,      // Use correct amount for valid pegin
            bitcoin_block_height,
            ethers::core::types::Bytes::from(serialized_valid_meta.clone()), // Clone valid meta
            ethers::core::types::Address::random(),
            eth_destination1, // Invalid Pegin (Dest 1)
            1_000_000_000_000_000_000u64.into(), // Amount irrelevant
            bitcoin_block_height,
            ethers::core::types::Bytes::from(serialized_invalid_meta_s2.clone()), // Clone invalid meta
            ethers::core::types::Address::random(),
        )
        .await
        .expect("multi_mint_two call (valid+invalid) should execute")
        .expect("Failed to get receipt for multi_mint_two call (valid+invalid)");

    it_info_print!("Scenario 2 Tx Receipt", tx_receipt_s2);
    assert_eq!(tx_receipt_s2.status.unwrap().as_u64(), 0, "Scenario 2 TX should revert");

    // Check balances & block heights unchanged (both should be unchanged due to revert)
    let balance3_after_s2 = botanix_eth_client.get_balance(eth_destination3).await.unwrap();
    let balance1_after_s2 = botanix_eth_client.get_balance(eth_destination1).await.unwrap();
    let height3_after_s2 = botanix_eth_client
        .mint_contract
        .pegin_bitcoin_block_height(eth_destination3)
        .await
        .unwrap();
    let height1_after_s2 = botanix_eth_client
        .mint_contract
        .pegin_bitcoin_block_height(eth_destination1)
        .await
        .unwrap();
    let mint_contract_balance_after_s2 = botanix_eth_client
        .get_botanix_balance(Address::from(mint_contract_address.0))
        .await
        .unwrap();
    assert_eq!(
        balance3_before_s2, balance3_after_s2,
        "Scenario 2 Balance 3 (Valid Target) should be unchanged"
    );
    assert_eq!(
        balance1_before_s2, balance1_after_s2,
        "Scenario 2 Balance 1 (Invalid Target) should be unchanged"
    );
    assert_eq!(
        height3_before_s2, height3_after_s2,
        "Scenario 2 Height 3 (Valid Target) should be unchanged"
    );
    assert_eq!(
        height1_before_s2, height1_after_s2,
        "Scenario 2 Height 1 (Invalid Target) should be unchanged"
    );
    assert_eq!(
        mint_contract_balance_after_s2, mint_contract_initial_balance,
        "Mint contract balance should be unchanged"
    );
    it_info_print!(
        "Scenario 2 Verified: Transaction reverted, balances and block heights unchanged."
    );

    // ==========================================
    // === Scenario 3: Invalid + Valid Pegin ===
    // ==========================================
    it_info_print!("Starting Scenario 3: Invalid Pegin + Valid Pegin");

    // Reuse the same valid/invalid metas from Scenario 2

    // Get initial balances & block heights (relative to after Scenario 2 completed)
    let balance1_before_s3 = botanix_eth_client.get_balance(eth_destination1).await.unwrap(); // Invalid Target
    let balance3_before_s3 = botanix_eth_client.get_balance(eth_destination3).await.unwrap(); // Valid Target
    let height1_before_s3 = botanix_eth_client
        .mint_contract
        .pegin_bitcoin_block_height(eth_destination1)
        .await
        .unwrap(); // Invalid Target
    let height3_before_s3 = botanix_eth_client
        .mint_contract
        .pegin_bitcoin_block_height(eth_destination3)
        .await
        .unwrap(); // Valid Target

    it_info_print!("Calling multiMintTwo with invalid then valid pegin...");
    let tx_receipt_s3 = botanix_eth_client
        .multi_mint_two(
            eth_destination1, // Invalid Pegin (Dest 1)
            1_000_000_000_000_000_000u64.into(), // Amount irrelevant
            bitcoin_block_height,
            ethers::core::types::Bytes::from(serialized_invalid_meta_s2.clone()), // Clone invalid meta first
            ethers::core::types::Address::random(),
            eth_destination3, // Valid Pegin (Dest 3)
            amount_wei3,      // Use correct amount for valid pegin
            bitcoin_block_height,
            ethers::core::types::Bytes::from(serialized_valid_meta.clone()), // Clone valid meta second
            ethers::core::types::Address::random(),
        )
        .await
        .expect("multi_mint_two call (invalid+valid) should execute")
        .expect("Failed to get receipt for multi_mint_two call (invalid+valid)");

    it_info_print!("Scenario 3 Tx Receipt", tx_receipt_s3);
    assert_eq!(tx_receipt_s3.status.unwrap().as_u64(), 0, "Scenario 3 TX should revert");

    // Check balances & block heights unchanged (both should be unchanged due to revert)
    let balance1_after_s3 = botanix_eth_client.get_balance(eth_destination1).await.unwrap();
    let balance3_after_s3 = botanix_eth_client.get_balance(eth_destination3).await.unwrap();
    let height1_after_s3 = botanix_eth_client
        .mint_contract
        .pegin_bitcoin_block_height(eth_destination1)
        .await
        .unwrap();
    let height3_after_s3 = botanix_eth_client
        .mint_contract
        .pegin_bitcoin_block_height(eth_destination3)
        .await
        .unwrap();
    let mint_contract_balance_after_s3 = botanix_eth_client
        .get_botanix_balance(Address::from(mint_contract_address.0))
        .await
        .unwrap();
    assert_eq!(
        balance1_before_s3, balance1_after_s3,
        "Scenario 3 Balance 1 (Invalid Target) should be unchanged"
    );
    assert_eq!(
        balance3_before_s3, balance3_after_s3,
        "Scenario 3 Balance 3 (Valid Target) should be unchanged"
    );
    assert_eq!(
        height1_before_s3, height1_after_s3,
        "Scenario 3 Height 1 (Invalid Target) should be unchanged"
    );
    assert_eq!(
        height3_before_s3, height3_after_s3,
        "Scenario 3 Height 3 (Valid Target) should be unchanged"
    );
    assert_eq!(
        mint_contract_balance_after_s3, mint_contract_initial_balance,
        "Mint contract balance should be unchanged"
    );
    it_info_print!(
        "Scenario 3 Verified: Transaction reverted, balances and block heights unchanged."
    );

    Ok(())
}
