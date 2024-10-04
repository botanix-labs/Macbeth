use std::{str::FromStr, time::Duration};

use bitcoin::{consensus::Encodable, Address};
use bitcoincore_rpc::RpcApi;
use btcserverlib::pegout_id::PegoutId;
use client::{BtcServerClient, SigningPackage, SigningPackageRequest};
use hex::{self, encode as hex_encode};
use rand::{rngs::StdRng, RngCore, SeedableRng};
use reth_chainspec::BOTANIX_TESTNET;
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
) -> Result<bitcoin::Transaction, Error> {
    let pegin_conf_depth = BOTANIX_TESTNET.parent_confirmation_depth;

    let coordinator_index = clients.len() - 1;
    let mut coordinator = clients.get(coordinator_index).cloned().unwrap();
    // First step: get the PSBT
    let checkpoint = {
        let tip = bitcoind.get_block_count().unwrap();
        bitcoind.get_block_hash(tip - pegin_conf_depth as u64).unwrap()
    };
    let utxo_merkle = coordinator
        .get_utxo_merkle_root(tonic::Request::new(client::Empty {}))
        .await
        .map_err(Error::Request)?
        .into_inner()
        .merkle_root;

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
        .await
        .expect("valid finalized request")
        .into_inner();

    let coord_psbt = bitcoin::Psbt::deserialize(&finalized.clone().psbt).unwrap();
    // TODO add some assertions for psbt here
    let final_tx = coord_psbt.clone().extract_tx().expect("valid tx");
    for (index, client) in clients.iter_mut().enumerate() {
        // skip the coordinator here
        if coordinator_index == index {
            continue;
        }

        client
            .signer_finalize(tonic::Request::new(client::FinalizeSignerRequest {
                psbt: finalized.clone().psbt,
            }))
            .await
            .map_err(Error::Request)?;

        // TODO Signers should end up with the same txs after they finalize
    }

    Ok(final_tx)
}

pub async fn test_many_inputs_signing(suite: &ConsensusIntegrationTestSuite) -> Result<(), Error> {
    let bitcoind = suite.global_context.bitcoind_rpc();
    // Load up the bitcoin wallet and generate some blocks
    for wallet in bitcoind.list_wallets().unwrap() {
        it_info_print!("#UNLOADING WALLET?", &wallet);
        let _ = bitcoind.unload_wallet(Some(&wallet));
    }
    let create_res = bitcoind.create_wallet(BITCOIND_WALLET_NAME, None, None, None, None);
    if create_res.is_err() {
        // wallet already exists, load wallet
        let _ = bitcoind.load_wallet(BITCOIND_WALLET_NAME);
    }
    let address = bitcoind.get_new_address(None, None).unwrap().assume_checked();
    // generate a block to the network looks live
    bitcoind.generate_to_address(202, &address).expect("generate to address");
    tokio::time::sleep(Duration::from_secs(5)).await;

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
        let err = pk.err().unwrap();
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
        let mut client = clients.get(0).cloned().unwrap();
        let res = client
            .get_gateway_address(tonic::Request::new(client::GetGatewayAddressRequest {
                eth_address: hex_encode(eth_address),
            }))
            .await
            .map_err(Error::Request)?
            .into_inner();
        let btc_address =
            Address::from_str(&res.gateway_address).expect("valid address").assume_checked();
        let txid = bitcoind
            .send_to_address(&btc_address, amount_to_send, None, None, None, None, None, None)
            .expect("send to address");

        // Generate some block to confirm it
        bitcoind.generate_to_address(2, &address).expect("generate to address");
        tokio::time::sleep(Duration::from_secs(1)).await;

        let tx_res = bitcoind.get_transaction(&txid, None).expect("valid tx");
        let pegin_tx = tx_res.transaction().expect("valid tx");
        let spk = btc_address.script_pubkey();
        let (vout, pegin_output) =
            pegin_tx.output.iter().enumerate().find(|(_, o)| o.script_pubkey == spk).unwrap();

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
            ot.txid.consensus_encode(&mut txid_bytes).unwrap();
            send_pegin_notification(
                c,
                pegin.btc_address.clone(),
                hex_encode(pegin.eth_address),
                txid_bytes.try_into().unwrap(),
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
    let pegout_id = PegoutId::from_bytes(&pegout_id_bytes).unwrap();

    let secp = bitcoin::secp256k1::Secp256k1::new();
    let sk = bitcoin::PrivateKey::generate(bitcoin::Network::Regtest);
    let pk = sk.public_key(&secp);

    let spk = bitcoin::Address::p2wpkh(&pk, bitcoin::Network::Regtest).unwrap().script_pubkey();

    // Notify some pending pegouts
    for c in clients.iter_mut() {
        // Each pegin is 100_000 satoshis, spending 100_000 should spend at least 2 inputs
        let amount = bitcoin::Amount::from_sat(100_000);
        send_pegout_notification(c, amount.to_sat(), 1, pegout_id, spk.clone()).await?;
    }

    let final_tx = do_signing(&mut clients, &bitcoind, &[0u8; 32]).await?;

    // Lets make sure it was broadcasted
    let tx_res = bitcoind.get_raw_transaction(&final_tx.txid(), None).expect("valid tx");
    it_info_print!("final tx_res: {:?}", tx_res);

    assert_eq!(final_tx.input.len(), 2);
    // One output should be the change output
    assert_eq!(final_tx.output.len(), 2);

    for c in clients.iter_mut() {
        let utxos = c
            .get_all_utxos(tonic::Request::new(client::Empty {}))
            .await
            .unwrap()
            .into_inner()
            .utxos;
        assert_eq!(utxos.len(), 5);
    }
    Ok(())
}
