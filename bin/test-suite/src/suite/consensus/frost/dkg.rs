use super::error::Error;
use crate::suite::consensus::ConsensusIntegrationTestSuite;
use bitcoin::{consensus::Encodable, Address, Amount, FeeRate, TxOut};
use client;
use std::{str::FromStr, vec};
use tonic::transport::Channel;

const _FEE_RATE: FeeRate = FeeRate::from_sat_per_vb_unchecked(30);

pub async fn dkg_flow(suite: &ConsensusIntegrationTestSuite) -> Result<(), Error> {
    let _secp = bitcoin::secp256k1::Secp256k1::new();

    // create clients
    let port = suite
        .local_context
        .btc_servers
        .as_ref()
        .and_then(|servers| servers.iter().nth(0).map(|val| val.port))
        .ok_or_else(|| Error::InvalidBtcServerPort)?;
    let mut c1 = client::BtcServerClient::connect(format!("http://localhost:{}", port))
        .await
        .map_err(Error::ServerConnect)?;

    let port = suite
        .local_context
        .btc_servers
        .as_ref()
        .and_then(|servers| servers.iter().nth(1).map(|val| val.port))
        .ok_or_else(|| Error::InvalidBtcServerPort)?;
    let mut c2 = client::BtcServerClient::connect(format!("http://localhost:{}", port))
        .await
        .map_err(Error::ServerConnect)?;

    let port = suite
        .local_context
        .btc_servers
        .as_ref()
        .and_then(|servers| servers.iter().nth(2).map(|val| val.port))
        .ok_or_else(|| Error::InvalidBtcServerPort)?;
    let mut c3 = client::BtcServerClient::connect(format!("http://localhost:{}", port))
        .await
        .map_err(Error::ServerConnect)?;

    let mut clients = vec![c1.clone(), c2.clone(), c3.clone()];

    // Getting public key should fail
    let pk = c1.get_public_key(tonic::Request::new(client::Empty {})).await;
    assert!(pk.is_err());
    let err = pk.err().unwrap();
    assert_eq!(err.code(), tonic::Code::Internal);
    assert_eq!(err.message(), "Failed to get public key: missing key package");
    let _ = do_dkg(&mut clients).await?;
    // After dkg we should be able to the dkg
    //// Get the pubkey
    let pk_1 = c1
        .get_public_key(tonic::Request::new(client::Empty {}))
        .await
        .map_err(Error::Request)?
        .into_inner();
    let pk_2 = c2
        .get_public_key(tonic::Request::new(client::Empty {}))
        .await
        .map_err(Error::Request)?
        .into_inner();
    let pk_3 = c3
        .get_public_key(tonic::Request::new(client::Empty {}))
        .await
        .map_err(Error::Request)?
        .into_inner();
    // Everyone got the same pks
    if !pk_1.publickey.eq(&pk_2.publickey) {
        return Err(Error::PublicKeyMismatch);
    }
    if !pk_1.publickey.eq(&pk_3.publickey) {
        return Err(Error::PublicKeyMismatch);
    }
    if !pk_2.publickey.eq(&pk_3.publickey) {
        return Err(Error::PublicKeyMismatch);
    }

    // Ensure all pks can be serialized as secp public keys
    let _ = bitcoin::secp256k1::PublicKey::from_str(&pk_1.publickey).map_err(Error::PubKeyParse)?;
    let _ = bitcoin::secp256k1::PublicKey::from_str(&pk_2.publickey).map_err(Error::PubKeyParse)?;
    let _ = bitcoin::secp256k1::PublicKey::from_str(&pk_3.publickey).map_err(Error::PubKeyParse)?;

    Ok(())
}

pub async fn do_dkg(clients: &mut Vec<client::BtcServerClient<Channel>>) -> Result<(), Error> {
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
        if p.payload.len() != 138 {
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

pub async fn send_pegin_notification(
    client: &mut client::BtcServerClient<Channel>,
    address: Address,
    eth_address: String,
    txid: [u8; 32],
) -> Result<(), Error> {
    let prev_out = TxOut {
        script_pubkey: address.script_pubkey(),
        value: Amount::from_sat(100_000_000).to_sat(),
    };

    let mut prev_out_bytes = Vec::new();
    prev_out.consensus_encode(&mut prev_out_bytes).unwrap();
    // Get a random 32 bytes

    let res = client
        .notify_pegin(tonic::Request::new(client::NotifyPeginRequest {
            eth_address,
            utxo_txid: hex::encode(txid),
            utxo_vout: 1,
            output: prev_out_bytes,
        }))
        .await;
    if res.is_err() {
        return Err(Error::PeginNotification);
    }
    Ok(())
}
