use super::error::Error;
use crate::{it_error_print, suite::consensus::ConsensusIntegrationTestSuite};
use bitcoin::{consensus::Encodable, Address, Amount};
use btcserverlib::pegout_id::PegoutId;
use client::{self, BtcServerClient};
use rand::{rngs::StdRng, RngCore, SeedableRng};
use std::{str::FromStr, vec};
use tonic::transport::Channel;

pub async fn dkg_flow(suite: &ConsensusIntegrationTestSuite) -> anyhow::Result<(), Error> {
    // create btc server clients
    let mut clients: Vec<BtcServerClient<Channel>> = vec![];
    for instance in 0..suite.global_context.instances {
        let port = suite
            .local_context
            .btc_processes
            .as_ref()
            .and_then(|process| process.iter().nth(instance as usize).map(|val| val.port))
            .ok_or_else(|| Error::InvalidBtcServerPort)?;
        let c = client::BtcServerClient::connect(format!("http://localhost:{}", port))
            .await
            .map_err(Error::ServerConnect)?;
        clients.push(c);
    }

    // Getting public key should fail for all clients
    for client in clients.iter_mut() {
        let pk = client.get_public_key(tonic::Request::new(client::Empty {})).await;
        assert!(pk.is_err());
        let err = pk.err().unwrap();
        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err.message().contains("missing key package"));
    }

    // now do the dkg
    do_dkg(&mut clients).await?;

    // Get the pubkey should succeed for all clients
    let mut pkeys: Vec<String> = vec![];
    for client in &mut clients {
        let pk = client
            .get_public_key(tonic::Request::new(client::Empty {}))
            .await
            .map_err(Error::Request)?
            .into_inner();
        // Ensure all pks can be serialized as secp public keys
        let _ =
            bitcoin::secp256k1::PublicKey::from_str(&pk.publickey).map_err(Error::PubKeyParse)?;
        pkeys.push(pk.publickey);
    }

    // Ensure everyone got the same pks
    pkeys.dedup();
    if pkeys.len() != 1 {
        return Err(Error::PublicKeyMismatch);
    }

    Ok(())
}

pub async fn do_dkg(clients: &mut [client::BtcServerClient<Channel>]) -> Result<(), Error> {
    // Round 1 dkg
    let mut round1_packages = vec![];
    for c in clients.iter_mut() {
        let p = c
            .get_round1_dkg_package(tonic::Request::new(client::Empty {}))
            .await
            .map_err(Error::Request)?
            .into_inner();
        round1_packages.push(p);
    }

    // Ensure all packages have correct props
    for p in round1_packages.iter() {
        if p.identifier.len() != 32 {
            return Err(Error::Round1PackagesLenghtMismatch);
        }
    }
    // Send each package to all other clients
    for (i, c) in clients.iter_mut().enumerate() {
        for (j, p) in round1_packages.iter().enumerate() {
            if i != j {
                c.new_round1_dkg_package(tonic::Request::new(client::DkgPayload {
                    identifier: p.identifier.clone(),
                    payload: p.payload.clone(),
                }))
                .await
                .map_err(Error::Request)?;
            }
        }
    }
    // Round 2 dkg
    let mut round2_packages = vec![];
    for c in clients.iter_mut() {
        let p = c
            .get_round2_dkg_package(tonic::Request::new(client::Empty {}))
            .await
            .map_err(Error::Request)?
            .into_inner();
        round2_packages.push(p);
    }
    // Ensure all packages have correct props
    // Not much to assert here, we can check the lenght of the ids
    for p in round2_packages.iter() {
        if p.identifier.len() != 32 {
            return Err(Error::Round2PackagesLenghtMismatch);
        }
    }

    // Send round 2 dkg packages to each respective participant
    for (i, c) in clients.iter_mut().enumerate() {
        for (j, p) in round2_packages.iter().enumerate() {
            if i != j {
                c.new_round2_dkg_package(tonic::Request::new(client::DkgPayload {
                    identifier: p.identifier.clone(),
                    payload: p.payload.clone(),
                }))
                .await
                .map_err(Error::Request)?;
            }
        }
    }
    Ok(())
}

// Uses random spk and pegout id
pub async fn send_pegout_notification(
    client: &mut client::BtcServerClient<Channel>,
    amount: u64,
    bitcoin_height: u64,
) -> Result<(), Error> {
    // Using stdRng here as it implements Send
    let mut rand = StdRng::from_entropy();
    let mut pegout_id_bytes = [0u8; 36];
    rand.fill_bytes(&mut pegout_id_bytes);
    let pegout_id = PegoutId::from_bytes(&pegout_id_bytes).unwrap();
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let sk = bitcoin::PrivateKey::generate(bitcoin::Network::Regtest);
    let pk = sk.public_key(&secp);

    let spk = bitcoin::Address::p2wpkh(&pk, bitcoin::Network::Regtest).unwrap().script_pubkey();
    let _ = client
        .notify_pegout(tonic::Request::new(client::NotifyPegoutRequest {
            pegout_id: pegout_id.clone().as_bytes().to_vec(),
            spk: spk.to_bytes().to_vec(),
            amount,
            height: bitcoin_height,
        }))
        .await
        .map_err(Error::PegoutNotification)?;
    Ok(())
}

pub async fn send_pegin_notification(
    client: &mut client::BtcServerClient<Channel>,
    address: Address,
    eth_address: String,
    txid: [u8; 32],
    amount: u64,
) -> Result<(), Error> {
    let mut prev_out_bytes = Vec::new();
    address.script_pubkey().consensus_encode(&mut prev_out_bytes).unwrap();
    let utxos = [client::Utxo {
        output: Some(client::TxOut {
            value: Amount::from_sat(amount).to_sat(),
            script_pubkey: Some(client::ScriptBuf { script: prev_out_bytes }),
        }),
        outpoint: Some(client::OutPoint { txid: txid.to_vec(), vout: 1 }),
        eth_address,
    }]
    .to_vec();

    let res =
        client.notify_pegins(tonic::Request::new(client::NotifyPeginsRequest { utxos })).await;
    if res.is_err() {
        return Err(Error::PeginNotification);
    }
    Ok(())
}

pub async fn send_pegins_notifications(
    client: &mut client::BtcServerClient<Channel>,
    txids: Vec<Vec<u8>>,
    eth_addresses: Vec<String>,
    btc_addresses: Vec<Address>,
    amounts: Vec<u64>,
) -> Result<(), Error> {
    assert_eq!(txids.len(), eth_addresses.len());
    assert_eq!(txids.len(), btc_addresses.len());
    assert_eq!(txids.len(), amounts.len());

    let mut utxos = Vec::new();
    for (i, txid) in txids.iter().enumerate() {
        let eth_address = eth_addresses[i].clone();
        let btc_address = btc_addresses[i].clone();
        let amount = amounts[i];

        let mut prev_out_bytes = Vec::new();
        btc_address.script_pubkey().consensus_encode(&mut prev_out_bytes).unwrap();
        utxos.push(client::Utxo {
            output: Some(client::TxOut {
                value: Amount::from_sat(amount).to_sat(),
                script_pubkey: Some(client::ScriptBuf { script: prev_out_bytes }),
            }),
            outpoint: Some(client::OutPoint { txid: txid.to_vec(), vout: 1 }),
            eth_address,
        });
    }

    let res =
        client.notify_pegins(tonic::Request::new(client::NotifyPeginsRequest { utxos })).await;
    if res.is_err() {
        it_error_print!("Pegin Error: {:?}", res.err());
        return Err(Error::PeginNotification);
    }
    Ok(())
}
