use std::str::FromStr;

use crate::{
    suite::{
        consensus::{
            frost::test_signing::{do_signing, Pegin},
            CreateTestConfig,
        },
        Suite,
    },
    utils::{
        get_checkpoint_block_hash, send_pegin_notification, send_pegout_notification,
        BlockChainInfoRes,
    },
};
use bitcoin::{consensus::Encodable, Address};
use bitcoin_hashes::Hash;
use bitcoincore_rpc::RpcApi;
use btcserverlib::pegout_id::PegoutId;
use hex::{self, encode as hex_encode};
use rand::{rngs::StdRng, RngCore, SeedableRng};
use reth_chainspec::BOTANIX_TESTNET;

use crate::{
    it_info_print,
    suite::consensus::{
        common::events::BITCOIND_WALLET_NAME,
        frost::{error::Error, test_dkg::do_dkg},
        ConsensusIntegrationTestSuite,
    },
    utils::generate_blocks,
};

const NUM_PEGINS: usize = 5;

pub async fn test_track_mempool(
    suite: &mut ConsensusIntegrationTestSuite,
) -> Result<(), anyhow::Error> {
    let bitcoind = suite.global_context.bitcoind_rpc();
    // Load up the bitcoin wallet and generate some blocks
    for wallet in bitcoind.list_wallets()? {
        it_info_print!("#UNLOADING WALLET?", &wallet);
        let _ = bitcoind.unload_wallet(Some(&wallet))?;
    }
    let create_res = bitcoind.create_wallet(BITCOIND_WALLET_NAME, None, None, None, None);
    if create_res.is_err() {
        tracing::info!("Wallet already exists, loading wallet ...");
        // wallet already exists, load wallet
        let _ = bitcoind.load_wallet(BITCOIND_WALLET_NAME);
    }
    generate_blocks(&bitcoind, 202).await;

    // create pegins container
    let mut pegins = vec![];

    // create btc server clients
    let mut clients = suite
        .local_context
        .btc_server_clients
        .clone()
        .expect("btc server rpc clients to be defined");

    // run the dkg
    do_dkg(&mut clients).await?;

    // pegin and pegout which will create, sign, and broadcast a psbt honoring pending pegouts
    // this will cause the signers to track the tx in their db
    // we will intentionally generate blocks while leaving the tracked tx in the mempool for testing

    let amount_to_send = bitcoin::Amount::from_sat(100_000);
    // create pegins
    for _ in 0..NUM_PEGINS {
        let eth_address = ethers::core::types::Address::random();
        // Lets get the gateway address for this eth address
        let mut client =
            clients.get(0).cloned().ok_or_else(|| anyhow::anyhow!("client not found"))?;
        let res = client
            .get_gateway_address(tonic::Request::new(client::GetGatewayAddressRequest {
                eth_address: hex_encode(eth_address),
            }))
            .await
            .map_err(Error::Request)?
            .into_inner();
        let btc_address = Address::from_str(&res.gateway_address)?.assume_checked();
        let txid = bitcoind.send_to_address(
            &btc_address,
            amount_to_send,
            None,
            None,
            None,
            None,
            None,
            None,
        )?;

        // Generate some block to confirm it
        generate_blocks(&bitcoind, 2).await;

        let tx_res = bitcoind.get_transaction(&txid, None)?;
        let pegin_tx = tx_res.transaction()?;
        let spk = btc_address.script_pubkey();
        let (vout, pegin_output) = pegin_tx
            .output
            .iter()
            .enumerate()
            .find(|(_, o)| o.script_pubkey == spk)
            .ok_or_else(|| anyhow::anyhow!("pegin output not found"))?;

        pegins.push(Pegin {
            eth_address,
            btc_address,
            outpoint: bitcoin::OutPoint { txid, vout: vout as u32 },
            amount: pegin_output.value,
        });
    }

    // get the aggregate pk from any of the clients
    // Here we are signing for inputs that are tweaked differently

    // Notify pegins to all peers
    let checkpoint_block_hash = get_checkpoint_block_hash(&bitcoind)?;
    for c in clients.iter_mut() {
        for pegin in pegins.iter() {
            let ot = pegin.outpoint;
            let mut txid_bytes = Vec::with_capacity(32);
            ot.txid.consensus_encode(&mut txid_bytes)?;
            send_pegin_notification(
                c,
                checkpoint_block_hash.clone(),
                pegin.btc_address.clone(),
                hex_encode(pegin.eth_address),
                txid_bytes.try_into().map_err(|_| anyhow::anyhow!("invalid txid"))?,
                ot.vout,
                pegin.amount.to_sat(),
            )
            .await?;
        }
    }

    let mut rand = StdRng::from_entropy();
    let mut pegout_id_bytes = [0u8; 36];
    rand.fill_bytes(&mut pegout_id_bytes);
    let pegout_id =
        PegoutId::from_bytes(&pegout_id_bytes).map_err(|_| anyhow::anyhow!("invalid pegout id"))?;

    let secp = bitcoin::secp256k1::Secp256k1::new();
    let sk = bitcoin::PrivateKey::generate(bitcoin::Network::Regtest);
    let pk = sk.public_key(&secp);

    let spk = pk.p2wpkh_script_code().expect("valid pk");

    // Notify there is a pending pegout
    for c in clients.iter_mut() {
        // Each pegin is 100_000 satoshis, spending 100_000 should spend at least 2 inputs
        send_pegout_notification(
            c,
            checkpoint_block_hash.clone(),
            amount_to_send.to_sat(),
            1,
            pegout_id,
            spk.clone(),
        )
        .await?;
    }
    // signs, broadcasts, and tracks the psbt honoring pending pegouts
    let tracked_tx = do_signing(&mut clients, &bitcoind, &[1u8; 32]).await?;

    // assert the pegout is not in the pending pegout list
    let pending_pegouts = clients[0]
        .get_pending_pegouts(tonic::Request::new(client::Empty {}))
        .await
        .expect("get pending pegouts")
        .into_inner();
    let pending_pegout_ids =
        pending_pegouts.pending_pegouts.iter().map(|p| p.pegout_id.clone()).collect::<Vec<_>>();
    println!("pending_pegouts: {:?}", pending_pegout_ids);
    assert!(!pending_pegout_ids.contains(&pegout_id_bytes.to_vec()));

    // kill bitcoind so mempool is cleared (conf is set to not persist mempool)
    suite
        .local_context
        .bitcoind_process
        .as_mut()
        .expect("bitcoind process to exist")
        .destroy_all_async()
        .await;

    // restart bitcoind
    let mut test_config = CreateTestConfig::default();
    test_config.create_bitcoind_node = true;
    suite.create_new_local_context(test_config).await?;

    // check the tx does not exist in the mempool
    let tx_ids = bitcoind.get_raw_mempool().expect("mempool should exist");
    assert!(!tx_ids.contains(&tracked_tx.txid()));

    // we have a tracked tx in the signers db but the tx doesn't exist in the mempool or in a block
    // now we can generate enough blocks so the tracked tx is older than the bitcoin checkpoint
    generate_blocks(&bitcoind, BOTANIX_TESTNET.parent_confirmation_depth).await;

    // sync the signers to the bitcoin checkpoint
    let deep_tip = bitcoind.call::<BlockChainInfoRes>("getblockchaininfo", &[]).unwrap().blocks -
        (BOTANIX_TESTNET.parent_confirmation_depth as u64);
    let deep_block_hash = bitcoind.get_block_hash(deep_tip).unwrap();

    let checkpoint_block_hash = get_checkpoint_block_hash(&bitcoind)?;
    for c in clients.iter_mut() {
        match c
            .new_consensus_checkpoint(client::ConsensusCheckpointRequest {
                checkpoint_block_hash: checkpoint_block_hash.clone(),
                pegins: vec![],
                pending_pegouts: vec![],
            })
            .await
        {
            Ok(_) => {}
            Err(e) => {
                it_info_print!("Error: {:?}", e);
                return Err(Error::ConsensusCheckpoint.into());
            }
        };
    }

    // when the signers synced their pegout schedulers,
    // they should have removed the tracked tx and added it back to the pending pegout list
    // since it's older than the checkpoint and doesn't exist in the mempool
    let pending_pegouts = clients[0]
        .get_pending_pegouts(tonic::Request::new(client::Empty {}))
        .await
        .expect("get pending pegouts")
        .into_inner();
    let pending_pegout_ids =
        pending_pegouts.pending_pegouts.iter().map(|p| p.pegout_id.clone()).collect::<Vec<_>>();
    println!("pending_pegouts: {:?}", pending_pegout_ids);
    // assert the pegout is in the pending pegout list
    assert!(pending_pegout_ids.contains(&pegout_id_bytes.to_vec()));

    Ok(())
}
