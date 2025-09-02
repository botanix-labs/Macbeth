use std::str::FromStr;

use bitcoin::{consensus::Encodable, Address};
use bitcoincore_rpc::RpcApi;
use btc_server_client::BtcServerClient;
use btcserverlib::pegout_id::PegoutId;
use hex::{self, encode as hex_encode};
use rand::{rngs::StdRng, Rng, RngCore, SeedableRng};
use tonic::transport::Channel;

use crate::{
    it_info_print,
    suite::consensus::{
        frost::{error::Error, test_dkg::do_dkg, test_signing::do_signing},
        ConsensusIntegrationTestSuite,
    },
    utils::{
        generate_blocks, get_checkpoint_block_hash, send_pegin_notification,
        send_pegout_notification, MIN_BLOCKS_COINBASE_MATURE,
    },
};

const COORDINATOR_INDEX: usize = 0;

// The pegin value is intentionally less than the pegout value, so that we can test the situation
// where the resultant tx weight is above the limit
const NUM_PEGINS: usize = 200;
const PEGIN_AMOUNT: u64 = 40_000;
const NUM_PENDING_PEGOUTS: usize = 110;
const PEGOUT_AMOUNT: u64 = 70_000;

pub struct Pegin {
    pub eth_address: ethers::core::types::Address,
    pub btc_address: bitcoin::Address,
    pub outpoint: bitcoin::OutPoint,
    pub amount: bitcoin::Amount,
}

/// Assert that all clients have the same UTXO set
pub async fn all_clients_have_same_wallet_state(
    _clients: &mut Vec<BtcServerClient<Channel>>,
) -> Result<(), Error> {
    // The coordinator will have a different state than the signers
    // This is b/c the signers are not tracking txs in the current implementation
    // Everntually they will all converge to the same state
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub async fn test_tx_weight_limit(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), anyhow::Error> {
    let mut rand = StdRng::from_entropy();
    let secp = bitcoin::secp256k1::Secp256k1::new();

    let bitcoind = suite.global_context.bitcoind_rpc();
    generate_blocks(&bitcoind, MIN_BLOCKS_COINBASE_MATURE).await;
    let address = bitcoind.get_new_address(None, None)?.assume_checked();

    // create btc server clients
    let mut clients = suite
        .local_context
        .btc_server_clients
        .clone()
        .expect("btc server rpc clients to be defined");

    // run the dkg
    do_dkg(&mut clients).await?;

    // create pegins
    let mut pegins = vec![];
    let amount_to_send = bitcoin::Amount::from_sat(PEGIN_AMOUNT);
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

    // Generate some block to confirm the pegins
    generate_blocks(&bitcoind, 2).await;

    // get the checkpoint blockhash
    let bitcoind = suite.global_context.bitcoind_rpc();
    let checkpoint_block_hash = get_checkpoint_block_hash(&bitcoind)?;

    // Notify pegins to all nodes
    // signers will not sign if they cannot locate the UTXOs they are being requested to sign
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

    // create lots of pending pegouts
    let mut original_pending_pegouts = vec![];
    for _ in 0..NUM_PENDING_PEGOUTS {
        let mut pegout_id_bytes = [0u8; 36];
        rand.fill_bytes(&mut pegout_id_bytes);
        let pegout_id = PegoutId::from_bytes(&pegout_id_bytes)
            .map_err(|_| anyhow::anyhow!("invalid pegout id"))?;
        let amount = bitcoin::Amount::from_sat(PEGOUT_AMOUNT);

        // create a new key for each pending pegout
        let sk = bitcoin::PrivateKey::generate(bitcoin::Network::Regtest);
        let pk = sk.public_key(&secp);
        let spk = pk.p2wpkh_script_code().expect("valid pk");

        original_pending_pegouts.push((pegout_id, amount, spk.clone(), pegout_id));
    }

    // Notify pending pegouts to all nodes
    for c in clients.iter_mut() {
        for pending_pegout in original_pending_pegouts.iter() {
            send_pegout_notification(
                c,
                checkpoint_block_hash.clone(),
                pending_pegout.1.to_sat(),
                1,
                pending_pegout.0,
                pending_pegout.2.clone(),
            )
            .await?;
        }
    }

    // this pegout should only contain a subset of the pending pegouts so as to stay within the
    // weight limit.
    let max_weight_pegout = do_signing(&mut clients, &bitcoind, &[2u8; 32]).await?;

    bitcoind.generate_to_address(10, &address).expect("generate regtest block");
    let first_tx_res =
        bitcoind.get_raw_transaction(&max_weight_pegout.compute_txid(), None).expect("valid tx");

    // update the checkpoint blockhash
    let checkpoint_block_hash = get_checkpoint_block_hash(&bitcoind)?;

    sync_checkpoint(&mut clients, checkpoint_block_hash.clone()).await?;

    assert_eq!(first_tx_res.compute_txid(), max_weight_pegout.compute_txid());
    assert!(max_weight_pegout.weight().to_wu() < btcserverlib::wallet::MAX_PEGOUT_TX_WEIGHT);
    it_info_print!(
        "First pegout tx weight: {} weight units (limit: {} weight units)",
        max_weight_pegout.weight().to_wu(),
        btcserverlib::wallet::MAX_PEGOUT_TX_WEIGHT
    );

    // check that all tx outputs (apart from the last change one) are pending pegouts.
    let pegged_out_spks: std::collections::HashSet<_> = first_tx_res
        .output
        .iter()
        .map(|o| o.script_pubkey.clone())
        .filter(|spk| original_pending_pegouts.iter().any(|p| p.2 == *spk))
        .collect();
    assert_eq!(pegged_out_spks.len(), first_tx_res.output.len() - 1);
    it_info_print!(format!(
        "First pegout tx processed {} pegouts out of {} pending pegouts, with {} inputs consumed",
        pegged_out_spks.len(),
        original_pending_pegouts.len(),
        first_tx_res.input.len()
    ));

    // check that the last output is the change output
    let expected_change_address = get_change_address(&mut clients).await?;
    let last_output_address = bitcoin::Address::from_script(
        &max_weight_pegout.output.last().unwrap().script_pubkey,
        bitcoin::Network::Regtest,
    )
    .unwrap();
    assert_eq!(last_output_address, expected_change_address);

    // check that the change utxo was added back into the utxo set
    let utxos = clients[COORDINATOR_INDEX]
        .get_all_utxos(tonic::Request::new(btc_server_client::Empty {}))
        .await
        .unwrap()
        .into_inner()
        .utxos;
    let no_eth_address_tweak_utxos =
        utxos.iter().filter(|u| u.eth_address.is_empty()).collect::<Vec<_>>();
    assert_eq!(no_eth_address_tweak_utxos.len(), 1);

    // check that the utxo set is correct. +1 for the new change output
    assert_eq!(utxos.len(), NUM_PEGINS - first_tx_res.input.len() + 1);

    // check that we have the right number of outputs remaining in the pending pegouts list
    let pending_pegouts_list = clients[COORDINATOR_INDEX]
        .get_pending_pegouts(tonic::Request::new(btc_server_client::Empty {}))
        .await?
        .into_inner()
        .pending_pegouts;
    assert_eq!(pending_pegouts_list.len(), original_pending_pegouts.len() - pegged_out_spks.len());

    // update the checkpoint blockhash
    let checkpoint_block_hash = get_checkpoint_block_hash(&bitcoind)?;

    // sync tx index to checkpoint block hash
    sync_checkpoint(&mut clients, checkpoint_block_hash.clone()).await?;

    // Generate a new pending pegout
    let mut pegout_id_bytes = [0u8; 36];
    rand.fill_bytes(&mut pegout_id_bytes);
    let pegout_id = PegoutId::from_bytes(&pegout_id_bytes).unwrap();
    let rand_amount = rand.gen_range(25_000..50_000); // Range: 25,000 to 49,999 sats
                                                      // get new a key
    let sk = bitcoin::PrivateKey::generate(bitcoin::Network::Regtest);
    let pk = sk.public_key(&secp);
    let spk = pk.p2wpkh_script_code().expect("valid pk");

    for c in clients.iter_mut() {
        send_pegout_notification(
            c,
            checkpoint_block_hash.clone(),
            rand_amount,
            1,
            pegout_id,
            spk.clone(),
        )
        .await?;
    }

    let second_pegout_tx = do_signing(&mut clients, &bitcoind, &[3u8; 32]).await?;
    bitcoind.generate_to_address(1, &address).expect("generate regtest block");

    let second_tx_res =
        bitcoind.get_raw_transaction(&second_pegout_tx.compute_txid(), None).expect("valid tx");

    // check that this second pegout contains the rest of the pending pegouts, as well as the one
    // just added above
    let second_pegged_out_spks: std::collections::HashSet<_> = second_tx_res
        .output
        .iter()
        .map(|o| o.script_pubkey.clone())
        .filter(|spk| original_pending_pegouts.iter().any(|p| p.2 == *spk))
        .collect();
    assert_eq!(second_pegged_out_spks.len(), second_tx_res.output.len() - 2);
    it_info_print!(
        "Second pegout tx processed {} remaining pegouts + 1 new pegout",
        second_pegged_out_spks.len()
    );

    // check that no pegout was included in both transactions
    assert_eq!(pegged_out_spks.intersection(&second_pegged_out_spks).count(), 0);

    // check that the last pegout was included
    let last_pegout_included = second_tx_res.output.iter().any(|o| o.script_pubkey == spk); // spk from the newly created pegout
    assert!(last_pegout_included);

    // check that the last output is the change output
    assert_eq!(
        second_tx_res.output.last().unwrap().script_pubkey,
        expected_change_address.script_pubkey()
    );

    // check that the pending pegout list is empty
    let pending_pegouts_list = clients[COORDINATOR_INDEX]
        .get_pending_pegouts(tonic::Request::new(btc_server_client::Empty {}))
        .await?
        .into_inner()
        .pending_pegouts;
    assert!(pending_pegouts_list.is_empty());
    it_info_print!(
        "All {} pegouts processed across 2 pegout txs",
        original_pending_pegouts.len() + 1
    );

    Ok(())
}

async fn get_change_address(
    clients: &mut Vec<BtcServerClient<Channel>>,
) -> anyhow::Result<bitcoin::Address> {
    let public_key_response = clients[COORDINATOR_INDEX]
        .get_public_key(tonic::Request::new(btc_server_client::Empty {}))
        .await?;
    let public_key_bytes = hex::decode(&public_key_response.into_inner().publickey)?;
    let public_key = secp256k1::PublicKey::from_slice(&public_key_bytes)?;
    let change_script =
        btcserverlib::wallet::address::generate_taproot_change_scriptpubkey(&public_key);
    let change_address = bitcoin::Address::from_script(&change_script, bitcoin::Network::Regtest)?;
    Ok(change_address)
}

async fn sync_checkpoint(
    clients: &mut [BtcServerClient<Channel>],
    checkpoint_block_hash: Vec<u8>,
) -> Result<(), Error> {
    for client in clients.iter_mut() {
        client
            .new_consensus_checkpoint(btc_server_client::ConsensusCheckpointRequest {
                checkpoint_block_hash: checkpoint_block_hash.clone(),
                pegins: vec![],
                pending_pegouts: vec![],
            })
            .await
            .map_err(|_| Error::ConsensusCheckpoint)?;
    }
    Ok(())
}
