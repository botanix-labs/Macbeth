use std::str::FromStr;

use crate::{
    suite::consensus::frost::test_signing::{do_signing, Pegin},
    utils::{get_checkpoint_block_hash, send_pegin_notification, send_pegout_notification},
};
use bitcoin::{consensus::Encodable, Address};
use bitcoincore_rpc::RpcApi;
use btcserverlib::pegout_id::PegoutId;
use hex::{self, encode as hex_encode};
use rand::{rngs::StdRng, RngCore, SeedableRng};

use crate::{
    it_info_print,
    suite::consensus::{
        frost::{error::Error, test_dkg::do_dkg},
        ConsensusIntegrationTestSuite,
    },
    utils::generate_blocks,
};

const NUM_PEGINS: usize = 5;

/// Test that the mempool is tracked correctly
///
/// Pegin then pegout which will:
/// create, sign, broadcast, and track a transaction but don't include it in a block.
/// Then restart bitcoind to clear the mempool of the tracked tx.
/// Then generate some blocks so the tracked tx is older than the checkpoint which should add
/// the tracked tx back to the pending pegout list since it's not in the mempool or in a block.
pub async fn test_track_mempool(
    suite: &mut ConsensusIntegrationTestSuite,
) -> Result<(), anyhow::Error> {
    // create btc server clients
    let mut clients = suite
        .local_context
        .btc_server_clients
        .clone()
        .expect("btc server rpc clients to be defined");

    // run the dkg
    do_dkg(&mut clients).await?;

    let bitcoind = suite.global_context.bitcoind_rpc();
    let mut pegins = vec![];
    let amount_to_send = bitcoin::Amount::from_sat(100_000);
    // create pegins
    for _ in 0..NUM_PEGINS {
        let eth_address = ethers::core::types::Address::random();
        // Lets get the gateway address for this eth address
        let mut client =
            clients.get(0).cloned().ok_or_else(|| anyhow::anyhow!("client not found"))?;
        let res = client
            .get_gateway_address(tonic::Request::new(btc_server_client::GetGatewayAddressRequest {
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

    let wpk = pk.wpubkey_hash().expect("valid wpubkey hash");
    let spk = bitcoin::ScriptBuf::new_p2wpkh(&wpk);

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
        .get_pending_pegouts(tonic::Request::new(btc_server_client::Empty {}))
        .await
        .expect("get pending pegouts")
        .into_inner();
    let pending_pegout_ids =
        pending_pegouts.pending_pegouts.iter().map(|p| p.pegout_id.clone()).collect::<Vec<_>>();
    assert!(!pending_pegout_ids.contains(&pegout_id_bytes.to_vec()));

    // we need to restart bitcoind so it drops it's mempool (drops the tracked tx)
    // then generate some blocks so the tracked_tx is older than the checkpoint but is not included
    // in a block

    // stop bitcoind
    let bitcoind_user = suite
        .local_context
        .bitcoind_node
        .as_ref()
        .expect("bitcoind node to exist")
        .bitcoind_user
        .clone();
    let bitcoind_password = suite
        .local_context
        .bitcoind_node
        .as_ref()
        .expect("bitcoind node to exist")
        .bitcoind_password
        .clone();
    suite
        .local_context
        .bitcoind_process
        .as_mut()
        .expect("bitcoind process to exist")
        .stop(&bitcoind_user, &bitcoind_password)
        .await;
    it_info_print!("Bitcoind stopped");

    // restart bitcoind
    let mut bitcoind_node =
        suite.local_context.bitcoind_node.take().expect("bitcoind node to exist");
    bitcoind_node.re_start(suite).await;
    suite.local_context.bitcoind_node = Some(bitcoind_node);
    it_info_print!("Bitcoind restarted");

    // check the tx does not exist in the mempool
    let tx_ids = bitcoind.get_raw_mempool().expect("mempool should exist");
    it_info_print!("tx_ids: {:?}", tx_ids.iter().map(|txid| hex::encode(txid)).collect::<Vec<_>>());
    assert!(!tx_ids.contains(&tracked_tx.compute_txid()));

    // assert there is still a tracked tx
    let tracked_txs = clients[0]
        .get_tracked_txs(tonic::Request::new(btc_server_client::Empty {}))
        .await
        .expect("get tracked txs")
        .into_inner()
        .tracked_txs;
    assert_eq!(tracked_txs.len(), 1);

    // generate some blocks so the tracked tx is older than the checkpoint
    generate_blocks(&bitcoind, 2).await;

    // sync the signers to the bitcoin checkpoint
    // signers will remove the tracked tx and add it back to the pending pegout list
    // since it's older than the checkpoint and doesn't exist in the mempool or in a block
    it_info_print!("Syncing signers to checkpoint");
    let checkpoint_block_hash = get_checkpoint_block_hash(&bitcoind)?;
    for c in clients.iter_mut() {
        match c
            .new_consensus_checkpoint(btc_server_client::ConsensusCheckpointRequest {
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

    // assert there are no tracked txs
    let tracked_txs = clients[0]
        .get_tracked_txs(tonic::Request::new(btc_server_client::Empty {}))
        .await
        .expect("get tracked txs")
        .into_inner()
        .tracked_txs;
    assert!(tracked_txs.is_empty());

    // assert the pegout is in the pending pegout list
    let pending_pegouts = clients[0]
        .get_pending_pegouts(tonic::Request::new(btc_server_client::Empty {}))
        .await
        .expect("get pending pegouts")
        .into_inner();
    let pending_pegout_ids =
        pending_pegouts.pending_pegouts.iter().map(|p| p.pegout_id.clone()).collect::<Vec<_>>();
    assert!(pending_pegout_ids.contains(&pegout_id_bytes.to_vec()));

    Ok(())
}
