pub mod btc_server;
pub mod dkg;
pub mod error;
pub mod signing;

use bitcoin::{consensus::Encodable, Amount, FeeRate, TxOut};
use client;
use error::Error;
use std::{str::FromStr, vec};
use tonic::transport::Channel;

const NETWORK: bitcoin::Network = bitcoin::Network::Signet;
const _FEE_RATE: FeeRate = FeeRate::from_sat_per_vb_unchecked(30);

async fn do_dkg(clients: &mut Vec<client::BtcServerClient<Channel>>) -> Result<(), Error> {
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
        assert_eq!(p.identifier.len(), 32);
        assert_eq!(p.payload.len(), 138);
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
        assert_eq!(p.identifier.len(), 32);
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

async fn send_pegin_notification(
    _secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::All>,
    client: &mut client::BtcServerClient<Channel>,
    eth_address: String,
    pk: String,
    txid: [u8; 32],
) {
    let address = reth_btc_wallet::address::gateway_address(
        &bitcoin::secp256k1::PublicKey::from_str(&pk).unwrap(),
        NETWORK,
    )
    .unwrap();
    let prev_out =
        TxOut { script_pubkey: address.script_pubkey(), value: Amount::from_sat(1000).to_sat() };

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
    assert!(res.is_ok());
}
