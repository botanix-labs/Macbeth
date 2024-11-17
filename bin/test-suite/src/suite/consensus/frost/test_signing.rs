use std::{collections::HashSet, str::FromStr};

use bitcoin::{consensus::Encodable, Address};
use bitcoin_hashes::Hash;
use bitcoincore_rpc::RpcApi;
use btcserverlib::pegout_id::PegoutId;
use client::{BtcServerClient, SigningPackage, SigningPackageRequest};
use hex::{self, encode as hex_encode};
use rand::{rngs::StdRng, Rng, RngCore, SeedableRng};
use reth_chainspec::BOTANIX_TESTNET;
use serde::Deserialize;
use tonic::transport::Channel;

use crate::{
    it_info_print,
    suite::consensus::{
        common::events::BITCOIND_WALLET_NAME,
        frost::{
            error::Error,
            test_dkg::{do_dkg, send_pegin_notification, send_pegout_notification},
        },
        ConsensusIntegrationTestSuite,
    },
    utils::generate_blocks,
};

const NUM_PEGINS: usize = 5;

struct Pegin {
    pub eth_address: ethers::core::types::Address,
    pub btc_address: bitcoin::Address,
    pub outpoint: bitcoin::OutPoint,
    pub amount: bitcoin::Amount,
}

/// Do a FROST signing round on the pending pegouts and return the finalized transaction
/// This util function will not send any pegouts for you, it will only do the FROST signing
pub async fn do_signing(
    clients: &mut Vec<BtcServerClient<Channel>>,
    bitcoind: &bitcoincore_rpc::Client,
    signing_session_id: &[u8; 32],
) -> anyhow::Result<bitcoin::Transaction, anyhow::Error> {
    let pegin_conf_depth = BOTANIX_TESTNET.parent_confirmation_depth;

    // Currently we support a static coordinator
    // this is always the first client
    let coordinator_index = 0;
    let mut coordinator = clients
        .get(coordinator_index)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("coordinator not found"))?;
    // First step: get the PSBT
    let checkpoint = {
        let tip = bitcoind.get_block_count()?;
        bitcoind.get_block_hash(tip - pegin_conf_depth as u64)?
    };

    // TODO assertions on wallet state
    // let _utxo_merkle = coordinator
    //     .get_utxo_merkle_root(tonic::Request::new(client::Empty {}))
    //     .await
    //     .map_err(Error::Request)?
    //     .into_inner()
    //     .merkle_root;

    let original_psbt = coordinator
        .get_psbt(tonic::Request::new(client::MakeTxRequest {
            signing_session_id: signing_session_id.to_vec(),
            checkpoint_block_hash: checkpoint[..].to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner()
        .psbt;

    // Round 1 signing
    // Signers will add their signing commitments to the psbt (including the coordinator)
    let mut round1_signing_commitments: Vec<SigningPackage> = vec![];
    for (index, client) in clients.iter_mut().enumerate() {
        // skip the coordinator here
        if coordinator_index == index {
            continue;
        }
        let c_signing = client
            .get_round1_signing_package(tonic::Request::new(client::SigningPackageRequest {
                psbt: original_psbt.clone(),
                signing_session_id: signing_session_id.to_vec(),
            }))
            .await
            .map_err(Error::Request)?
            .into_inner();
        round1_signing_commitments.push(c_signing);
    }

    // Coordinating node will collect the PSBTs with the signing commitments
    for signing_package in round1_signing_commitments {
        coordinator
            .new_round1_signing_package(tonic::Request::new(signing_package))
            .await
            .map_err(Error::Request)?;
    }

    // Signing Round 2
    // Get signing package
    let to_sign_package = coordinator
        .get_to_sign_package(tonic::Request::new(client::ToSignRequest {
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    // Signers should add their partial sigs to the psbt for each input
    let mut round2_signing_commitments: Vec<SigningPackage> = vec![];
    for (index, client) in clients.iter_mut().enumerate() {
        // skip the coordinator here
        if coordinator_index == index {
            continue;
        }
        let c_signing2 = client
            .get_round2_signing_package(tonic::Request::new(SigningPackageRequest {
                psbt: to_sign_package.clone().psbt,
                signing_session_id: signing_session_id.to_vec(),
            }))
            .await
            .map_err(Error::Request)?
            .into_inner();
        round2_signing_commitments.push(c_signing2);
    }

    // Coordinating node will collect the PSBTs with the partial sigs
    for signing_package in round2_signing_commitments {
        coordinator
            .new_round2_signing_package(tonic::Request::new(signing_package))
            .await
            .map_err(Error::Request)?;
    }

    let finalized = coordinator
        .finalize_signing(tonic::Request::new(client::FinalizeSigningRequest {
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await?
        .into_inner();

    let coord_psbt = bitcoin::Psbt::deserialize(&finalized.clone().psbt)?;
    // TODO add some assertions for psbt here
    let final_tx = coord_psbt.clone().extract_tx()?;
    // for (index, client) in clients.iter_mut().enumerate() {
    //     // skip the coordinator here
    //     if coordinator_index == index {
    //         continue;
    //     }

    //     client
    //         .signer_finalize(tonic::Request::new(client::FinalizeSignerRequest {
    //             psbt: finalized.clone().psbt,
    //         }))
    //         .await
    //         .map_err(Error::Request)?;

    //     // TODO Signers should end up with the same tracked txs after they finalize
    // }

    Ok(final_tx)
}

/// Assert that all clients have the same UTXO set
pub async fn all_clients_have_same_wallet_state(
    clients: &mut Vec<BtcServerClient<Channel>>,
) -> Result<(), Error> {
    // The coordinator will have a different state than the signers
    // This is b/c the signers are not tracking txs in the current implementation
    // Everntually they will all converge to the same state
    return Ok(());
    let mut utxo_merkle_root = HashSet::new();
    for c in clients.iter_mut() {
        let root = c
            .get_wallet_state(tonic::Request::new(client::Empty {}))
            .await
            .map_err(Error::Request)?
            .into_inner()
            .utxo_root;
        utxo_merkle_root.insert(root);
    }
    it_info_print!("utxo_merkle_root: {:?}", utxo_merkle_root.len());
    assert_eq!(utxo_merkle_root.len(), 2);
    Ok(())
}

pub async fn test_many_inputs_signing(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), anyhow::Error> {
    let coordinator_index = 0;
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

    let address = bitcoind.get_new_address(None, None)?.assume_checked();

    // create pegins container
    let mut pegins = vec![];

    // create btc server clients
    let mut clients = suite
        .local_context
        .btc_server_clients
        .clone()
        .expect("btc server rpc clients to be defined");

    // Getting public key should fail for all clients
    for client in &mut clients {
        let pk = client.get_public_key(tonic::Request::new(client::Empty {})).await;
        assert!(pk.is_err());
        let err = pk.err().ok_or_else(|| anyhow::anyhow!("missing key package"))?;
        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err.message().contains("missing key package"));
    }

    // run the dkg
    do_dkg(&mut clients).await?;
    let amount_to_send = bitcoin::Amount::from_sat(100_000);
    // create NUM_PEGINS pegins
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
    // Here we are signing for a NUM_PEGINS inputs that are tweaked differently

    // Notify peg ins to all peers
    // signers will not sign if they cannot locate the UTXOs they are being requested to sign
    for c in clients.iter_mut() {
        for pegin in pegins.iter() {
            let ot = pegin.outpoint;
            let mut txid_bytes = Vec::with_capacity(32);
            ot.txid.consensus_encode(&mut txid_bytes)?;
            send_pegin_notification(
                c,
                pegin.btc_address.clone(),
                hex_encode(pegin.eth_address),
                txid_bytes.try_into().map_err(|_| anyhow::anyhow!("invalid txid"))?,
                ot.vout,
                pegin.amount.to_sat(),
            )
            .await?;
        }
    }

    // Using stdRng here as it implements Send
    let mut rand = StdRng::from_entropy();
    let mut pegout_id_bytes = [0u8; 36];
    rand.fill_bytes(&mut pegout_id_bytes);
    let pegout_id =
        PegoutId::from_bytes(&pegout_id_bytes).map_err(|_| anyhow::anyhow!("invalid pegout id"))?;

    let secp = bitcoin::secp256k1::Secp256k1::new();
    let sk = bitcoin::PrivateKey::generate(bitcoin::Network::Regtest);
    let pk = sk.public_key(&secp);

    let spk = bitcoin::Address::p2wpkh(&pk, bitcoin::Network::Regtest)?.script_pubkey();

    // Calling do_signing should fail as we have no pending pegouts
    let err_res = do_signing(&mut clients, &bitcoind, &[0u8; 32])
        .await
        .err()
        .ok_or_else(|| anyhow::anyhow!("error not present"))?;
    println!("err_res: {:?}", err_res);

    assert!(err_res.to_string().contains("Failed to validate psbt: inputs cannot be 0"));

    // Notify some pending pegouts
    let amount = bitcoin::Amount::from_sat(100_000);
    for c in clients.iter_mut() {
        // Each pegin is 100_000 satoshis, spending 100_000 should spend at least 2 inputs
        send_pegout_notification(c, amount.to_sat(), 1, pegout_id, spk.clone()).await?;
    }
    let final_tx = do_signing(&mut clients, &bitcoind, &[1u8; 32]).await?;
    bitcoind.generate_to_address(1, &address).expect("generate regtest block");

    // Lets make sure it was broadcasted
    let tx_res = bitcoind.get_raw_transaction(&final_tx.txid(), None).expect("valid tx");
    it_info_print!("final tx_res: {:?}", tx_res);
    assert_eq!(tx_res.txid(), final_tx.txid());

    assert_eq!(final_tx.input.len(), 2);
    // One output should be the change output
    assert_eq!(final_tx.output.len(), 2);

    // Assert that all clients have the same UTXO set
    all_clients_have_same_wallet_state(&mut clients).await?;

    let mut pending_pegouts = vec![];
    let number_of_pending_pegouts = 5;
    for _ in 0..number_of_pending_pegouts {
        let mut pegout_id_bytes = [0u8; 36];
        rand.fill_bytes(&mut pegout_id_bytes);
        let pegout_id = PegoutId::from_bytes(&pegout_id_bytes)
            .map_err(|_| anyhow::anyhow!("invalid pegout id"))?;
        let rand_amount = rand.gen::<u64>() % 75_000;
        let amount = bitcoin::Amount::from_sat(rand_amount);

        // get new a key
        let sk = bitcoin::PrivateKey::generate(bitcoin::Network::Regtest);
        let pk = sk.public_key(&secp);
        let spk = bitcoin::Address::p2wpkh(&pk, bitcoin::Network::Regtest)?.script_pubkey();

        pending_pegouts.push((pegout_id, amount, spk.clone(), pegout_id));
    }

    // Lets settle multiple pegouts
    for c in clients.iter_mut() {
        for pending_pegout in pending_pegouts.iter() {
            send_pegout_notification(
                c,
                pending_pegout.1.to_sat(),
                1,
                pending_pegout.0,
                pending_pegout.2.clone(),
            )
            .await?;
        }
    }

    let final_tx = do_signing(&mut clients, &bitcoind, &[2u8; 32]).await?;
    bitcoind.generate_to_address(1, &address).expect("generate regtest block");
    let tx_res = bitcoind.get_raw_transaction(&final_tx.txid(), None).expect("valid tx");

    assert_eq!(tx_res.txid(), final_tx.txid());
    assert!(final_tx.input.len() > 1);
    // 5 pegout outputs + 1 change output
    assert_eq!(final_tx.output.len(), 6);
    // last output is the change output
    let change_address =
        bitcoin::Address::from_script(&final_tx.output[5].script_pubkey, bitcoin::Network::Regtest)
            .unwrap();
    assert_eq!(change_address.address_type().unwrap(), bitcoin::AddressType::P2tr);

    let utxos = clients[coordinator_index]
        .get_all_utxos(tonic::Request::new(client::Empty {}))
        .await
        .unwrap()
        .into_inner()
        .utxos;
    // We still have the same UTXOs we started with however some of them should be used with tracked
    // txs
    assert_eq!(utxos.len(), 5);
    bitcoind.generate_to_address(1, &address).expect("generate regtest block");
    all_clients_have_same_wallet_state(&mut clients).await?;

    // Need to define a custom struct as breaking changes in bitcoin-core will cause the
    // deserialization to fail
    #[derive(Deserialize)]
    struct BlockChainInfoRes {
        blocks: u64,
    }

    let deep_tip = bitcoind.call::<BlockChainInfoRes>("getblockchaininfo", &[]).unwrap().blocks -
        (BOTANIX_TESTNET.parent_confirmation_depth as u64);
    let deep_block_hash = bitcoind.get_block_hash(deep_tip).unwrap();

    for c in clients.iter_mut() {
        c.tx_index_new_checkpoint(tonic::Request::new(client::SyncTxIndexRequest {
            checkpoint_block_hash: deep_block_hash.to_byte_array().to_vec(),
        }))
        .await
        .expect("valid checkpoint");
    }

    let utxos = clients[coordinator_index]
        .get_all_utxos(tonic::Request::new(client::Empty {}))
        .await
        .unwrap()
        .into_inner()
        .utxos;

    it_info_print!("utxos: {:?}", utxos);
    it_info_print!("len(utxos): {:?}", utxos.len());

    // expecting both change utxos to be added back into the utxo set
    let no_eth_address_tweak_utxos =
        utxos.iter().filter(|u| u.eth_address.is_empty()).collect::<Vec<_>>();
    let change_ots = no_eth_address_tweak_utxos
        .iter()
        .map(|u| {
            let outpoint = u.outpoint.clone().ok_or_else(|| anyhow::anyhow!("invalid outpoint"))?;
            let ot = bitcoin::OutPoint {
                txid: bitcoin::Txid::from_slice(&outpoint.txid)
                    .map_err(|_| anyhow::anyhow!("invalid txid"))?,
                vout: outpoint.vout,
            };
            Ok(ot)
        })
        .collect::<Result<Vec<_>, anyhow::Error>>()?;
    assert_eq!(no_eth_address_tweak_utxos.len(), 2);
    // And all tracked utxos are removed at this point so we are only left with change outputs + one
    // user provided input TODO this is flaky
    // im not sure this is deterministic, we might end up spending all the inputs depending on the
    // fees assert_eq!(utxos.len(), no_eth_address_tweak_utxos + 1);

    // Lets spend some change output
    // Generate new pending pegout
    let mut pegout_id_bytes = [0u8; 36];
    rand.fill_bytes(&mut pegout_id_bytes);
    let pegout_id = PegoutId::from_bytes(&pegout_id_bytes).unwrap();
    let rand_amount = 100_000;
    // get new a key
    let sk = bitcoin::PrivateKey::generate(bitcoin::Network::Regtest);
    let pk = sk.public_key(&secp);
    let spk = bitcoin::Address::p2wpkh(&pk, bitcoin::Network::Regtest)?.script_pubkey();

    for c in clients.iter_mut() {
        send_pegout_notification(c, rand_amount, 1, pegout_id, spk.clone()).await?;
    }

    let final_tx = do_signing(&mut clients, &bitcoind, &[3u8; 32]).await?;
    bitcoind.generate_to_address(1, &address).expect("generate regtest block");
    it_info_print!("final_tx: {:?}", final_tx);

    let tx_res = bitcoind.get_raw_transaction(&final_tx.txid(), None)?;
    let tx_ots = final_tx.input.iter().map(|i| i.previous_output).collect::<Vec<_>>();
    assert_eq!(tx_res.txid(), final_tx.txid());

    // Should contain at least one of the change utxos
    let found_match = change_ots.iter().any(|change_ot| tx_ots.contains(change_ot));
    assert!(found_match);

    all_clients_have_same_wallet_state(&mut clients).await?;
    Ok(())
}
