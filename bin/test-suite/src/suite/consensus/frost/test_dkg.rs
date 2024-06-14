use super::error::Error;
use crate::suite::consensus::ConsensusIntegrationTestSuite;
use bitcoin::{consensus::Encodable, Address, Amount, FeeRate, TxOut};
use client::{self, BtcServerClient};
use std::{str::FromStr, vec};
use tonic::transport::Channel;

pub async fn dkg_flow(suite: &ConsensusIntegrationTestSuite) -> Result<(), Error> {
    // create btc server clients
    let mut clients: Vec<BtcServerClient<Channel>> = vec![];
    for instance in 0..suite.global_context.instances {
        let port = suite
            .local_context
            .btc_servers
            .as_ref()
            .and_then(|servers| servers.iter().nth(instance as usize).map(|val| val.port))
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

pub async fn send_pegin_notification(
    client: &mut client::BtcServerClient<Channel>,
    address: Address,
    eth_address: String,
    txid: [u8; 32],
) -> Result<(), Error> {
    let prev_out =
        TxOut { script_pubkey: address.script_pubkey(), value: Amount::from_sat(100_000_000) };

    let mut prev_out_bytes = Vec::new();
    prev_out.consensus_encode(&mut prev_out_bytes).unwrap();

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
