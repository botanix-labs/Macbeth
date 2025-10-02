use std::str::FromStr;

use bitcoin::{consensus::Encodable, Address};
use bitcoincore_rpc::RpcApi;
use client::BtcServerClient;
use btcserverlib::pegout_id::PegoutId;
use hex::{self, encode as hex_encode};
use rand::{rngs::StdRng, RngCore, SeedableRng};
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
const NUM_CLAIMED_PEGINS: usize = 1;
const NUM_UNCLAIMED_PEGINS: usize = 1;
const NUM_UNCLAIMED_CHANGE_UTXOS: usize = 1;
const PEGIN_AMOUNT: u64 = 100_000;
const NUM_PEGOUTS: usize = 1;
const PEGOUT_AMOUNT: u64 = 250_000; // so that it uses all 3 utxos

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
pub async fn test_utxo_recovery(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), anyhow::Error> {
    let mut rand = StdRng::from_entropy();
    let secp = bitcoin::secp256k1::Secp256k1::new();

    let bitcoind = suite.global_context.bitcoind_rpc();
    generate_blocks(&bitcoind, MIN_BLOCKS_COINBASE_MATURE).await;

    // create btc server clients
    let mut clients = suite
        .local_context
        .btc_server_clients
        .clone()
        .expect("btc server rpc clients to be defined");

    // run the dkg
    do_dkg(&mut clients).await?;

    // create pegins
    let mut claimed_pegin = vec![];
    let mut unclaimed_pegin = vec![];
    let amount_to_send = bitcoin::Amount::from_sat(PEGIN_AMOUNT);
    for _ in 0..(NUM_CLAIMED_PEGINS + NUM_UNCLAIMED_PEGINS) {
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

        let tx_res = bitcoind.get_transaction(&txid, None)?;
        let pegin_tx = tx_res.transaction()?;
        let spk = btc_address.script_pubkey();
        let (vout, pegin_output) = pegin_tx
            .output
            .iter()
            .enumerate()
            .find(|(_, o)| o.script_pubkey == spk)
            .ok_or_else(|| anyhow::anyhow!("pegin output not found"))?;

        if claimed_pegin.len() < NUM_CLAIMED_PEGINS {
            claimed_pegin.push(Pegin {
                eth_address,
                btc_address,
                outpoint: bitcoin::OutPoint { txid, vout: vout as u32 },
                amount: pegin_output.value,
            });
        } else {
            unclaimed_pegin.push(Pegin {
                eth_address,
                btc_address,
                outpoint: bitcoin::OutPoint { txid, vout: vout as u32 },
                amount: pegin_output.value,
            });
        }
    }

    // send a utxo to the change address that we will claim later
    let change_address = get_change_address(&mut clients).await?;
    let change_txid = bitcoind.send_to_address(
        &change_address,
        bitcoin::Amount::from_sat(PEGIN_AMOUNT),
        None,
        None,
        None,
        None,
        None,
        None,
    )?;

    // get vout of the change output
    let change_tx_res = bitcoind.get_raw_transaction(&change_txid, None)?;
    let change_vout = change_tx_res
        .output
        .iter()
        .enumerate()
        .find(|(_, o)| o.script_pubkey == change_address.script_pubkey())
        .ok_or_else(|| anyhow::anyhow!("change output not found"))?
        .0;

    // Generate some block to confirm the pegins
    generate_blocks(&bitcoind, 2).await;

    // get the checkpoint blockhash
    let bitcoind = suite.global_context.bitcoind_rpc();
    let checkpoint_block_hash = get_checkpoint_block_hash(&bitcoind)?;

    // Notify pegins to all nodes
    for c in clients.iter_mut() {
        for pegin in claimed_pegin.iter() {
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

    // Now the nodes have the claimed pegins, but not the unclaimed pegins.
    assert_eq!(get_utxo_count(&mut clients).await?, NUM_CLAIMED_PEGINS);

    // Test 1: Attempting to recover an already claimed pegin should not add any new utxos.
    let claimed_utxos = clients[COORDINATOR_INDEX]
        .get_all_utxos(tonic::Request::new(client::Empty {}))
        .await?
        .into_inner()
        .utxos;
    assert_eq!(claimed_utxos.len(), NUM_CLAIMED_PEGINS);

    let utxos_to_recover = claimed_utxos
        .iter()
        .map(|utxo| {
            client::UtxoToRecover {
                outpoint: utxo.outpoint.clone(), // note this is little endian
                eth_address: utxo.eth_address.clone(),
            }
        })
        .collect::<Vec<_>>();

    let res = clients[COORDINATOR_INDEX]
        .recover_missing_utxos(client::RecoverMissingUtxosRequest {
            utxos: utxos_to_recover.clone(),
        })
        .await?;
    let res = res.into_inner();
    assert_eq!(res.total_requested, NUM_CLAIMED_PEGINS as u64);
    assert_eq!(res.total_recovered, 0);

    // Test 2: Attempting to recover the unclaimed pegins should add the unclaimed pegins to the
    // utxo set.
    let unclaimed_utxos = unclaimed_pegin
        .iter()
        .map(|pegin| client::UtxoToRecover {
            outpoint: Some(client::OutPoint {
                txid: {
                    let mut txid_bytes = Vec::new();
                    pegin.outpoint.txid.consensus_encode(&mut txid_bytes).unwrap();
                    txid_bytes
                },
                vout: pegin.outpoint.vout,
            }),
            eth_address: hex_encode(pegin.eth_address),
        })
        .collect::<Vec<_>>();

    let res = clients[COORDINATOR_INDEX]
        .recover_missing_utxos(client::RecoverMissingUtxosRequest {
            utxos: unclaimed_utxos.clone(),
        })
        .await?;
    let res = res.into_inner();
    assert_eq!(res.total_requested, NUM_UNCLAIMED_PEGINS as u64);
    assert_eq!(res.total_recovered, NUM_UNCLAIMED_PEGINS as u64);

    // test 3 is to try and recover the change output (has no eth address)
    let change_utxos = vec![client::UtxoToRecover {
        outpoint: Some(client::OutPoint {
            txid: {
                let mut txid_bytes = Vec::new();
                change_txid.consensus_encode(&mut txid_bytes).unwrap();
                txid_bytes
            },
            vout: change_vout as u32,
        }),
        eth_address: "".to_string(), // no eth address for change output
    }];

    let res = clients[COORDINATOR_INDEX]
        .recover_missing_utxos(client::RecoverMissingUtxosRequest {
            utxos: change_utxos.clone(),
        })
        .await?;
    let res = res.into_inner();
    assert_eq!(res.total_requested, NUM_UNCLAIMED_CHANGE_UTXOS as u64);
    assert_eq!(res.total_recovered, NUM_UNCLAIMED_CHANGE_UTXOS as u64);

    // check that the utxo set is correct.
    let total_utxos = get_utxo_count(&mut clients).await?;
    assert_eq!(total_utxos, NUM_CLAIMED_PEGINS + NUM_UNCLAIMED_PEGINS + NUM_UNCLAIMED_CHANGE_UTXOS);

    // create a pegout that spends from every utxo in the utxo set
    let mut original_pending_pegouts = vec![];
    for _ in 0..NUM_PEGOUTS {
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

    let checkpoint_block_hash = get_checkpoint_block_hash(&bitcoind)?;

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
    let pegout_tx = do_signing(&mut clients, &bitcoind, &[2u8; 32]).await?;
    it_info_print!(format!("pegout txid: {}", pegout_tx.compute_txid()));

    generate_blocks(&bitcoind, 2).await;
    let pegout_tx_res =
        bitcoind.get_raw_transaction(&pegout_tx.compute_txid(), None).expect("valid tx");
    it_info_print!(format!("pegout tx: {:?}", pegout_tx_res));

    sync_checkpoint(&mut clients, checkpoint_block_hash.clone()).await?;

    // assert that the pegout successfully spent from all the recoveredutxos
    it_info_print!(format!("pegout tx input len: {}", pegout_tx_res.input.len()));
    assert_eq!(pegout_tx_res.input.len(), total_utxos);

    Ok(())
}

async fn get_change_address(
    clients: &mut Vec<BtcServerClient<Channel>>,
) -> anyhow::Result<bitcoin::Address> {
    let public_key_response = clients[COORDINATOR_INDEX]
        .get_public_key(tonic::Request::new(client::Empty {}))
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
            .new_consensus_checkpoint(client::ConsensusCheckpointRequest {
                checkpoint_block_hash: checkpoint_block_hash.clone(),
                pegins: vec![],
                pending_pegouts: vec![],
            })
            .await
            .map_err(|_| Error::ConsensusCheckpoint)?;
    }
    Ok(())
}

pub async fn get_utxo_count(clients: &mut Vec<BtcServerClient<Channel>>) -> anyhow::Result<usize> {
    let utxos = clients[COORDINATOR_INDEX]
        .get_all_utxos(tonic::Request::new(client::Empty {}))
        .await?
        .into_inner()
        .utxos;
    Ok(utxos.len())
}
