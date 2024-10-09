use std::{collections::HashSet, str::FromStr};

use super::{
    error::Error,
    test_dkg::{do_dkg, send_pegin_notification},
};
use crate::{
    it_info_print,
    suite::consensus::{frost::test_dkg::send_pegins_notifications, ConsensusIntegrationTestSuite},
};
use bitcoin::{hashes::Hash, Address};
use hex::{self, encode as hex_encode};

const NUM_UTXOS: usize = 10;

pub struct Pegins {
    pub eth_addresses: Vec<ethers::core::types::Address>,
    pub btc_addresses: Vec<Address>,
    pub txids: Vec<[u8; 32]>,
}

impl Pegins {
    pub fn new() -> Self {
        Pegins { eth_addresses: Vec::new(), btc_addresses: Vec::new(), txids: Vec::new() }
    }
}

pub async fn test_utxo_commitment(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), Error> {
    // create pegins container
    let mut pegins = Pegins::new();

    // create NUM_UTXOS pegins
    for _ in 0..NUM_UTXOS {
        let eth_address = ethers::core::types::Address::random();
        pegins.eth_addresses.push(eth_address);
        pegins.txids.push(rand::random::<[u8; 32]>());
    }

    // create btc server clients
    let mut clients =
        suite.local_context.btc_server_clients.as_ref().expect("btc servers not found").clone();
    // run the dkg
    let _ = do_dkg(&mut clients).await?;

    // get the aggregate pk from any of the clients
    // Here we are signing for a INPUTS_TO_SPEND inputs that are tweaked differently
    for input in 0..NUM_UTXOS {
        let eth_address = pegins.eth_addresses.get(input).cloned().unwrap();
        let pk = clients[0]
            .get_gateway_address(tonic::Request::new(client::GetGatewayAddressRequest {
                eth_address: hex_encode(eth_address),
            }))
            .await
            .map_err(Error::Request)?
            .into_inner();
        let btc_address =
            Address::from_str(&pk.gateway_address).expect("valid address").assume_checked();
        pegins.btc_addresses.push(btc_address);
    }

    // Notify peg ins to all peers
    // signers will not sign if they cannot locate the UTXOs they are being requested to sign
    for c in clients.iter_mut() {
        for input in 0..NUM_UTXOS {
            let txid = pegins.txids.get(input).cloned().unwrap();
            let eth_address = pegins.eth_addresses.get(input).cloned().unwrap();
            let btc_address = pegins.btc_addresses.get(input).cloned().unwrap();
            let _ = send_pegin_notification(
                c,
                btc_address.clone(),
                hex_encode(eth_address),
                txid,
                1, // vout
                100_000_000,
            )
            .await?;
        }
    }
    let mut hashset = HashSet::new();
    for c in clients.iter_mut() {
        let utxo_set = c
            .get_utxo_merkle_root(tonic::Request::new(client::Empty {}))
            .await
            .unwrap()
            .into_inner()
            .merkle_root;
        hashset.insert(utxo_set);
    }
    // all the btc_servers should have the same merkel commitment to the utxo set
    assert_eq!(hashset.len(), 1);

    // Get all utxos
    let mut all_utxos = vec![];
    for c in clients.iter_mut() {
        let utxos = c
            .get_all_utxos(tonic::Request::new(client::Empty {}))
            .await
            .unwrap()
            .into_inner()
            .utxos;
        all_utxos.push(utxos);
    }
    all_utxos.dedup();
    assert_eq!(all_utxos.len(), 1);
    // all the btc_servers should have the same utxos
    let utxos = clients[0]
        .get_all_utxos(tonic::Request::new(client::Empty {}))
        .await
        .unwrap()
        .into_inner()
        .utxos;
    // All the utxos are accounted for
    assert_eq!(utxos.len(), NUM_UTXOS);
    // There should be dups
    let mut deduped = utxos.clone();
    deduped.dedup();
    assert_eq!(utxos.len(), deduped.len());

    let txids = pegins
        .txids
        .iter()
        .map(|txid| bitcoin::Txid::from_slice(txid).unwrap())
        .collect::<Vec<bitcoin::Txid>>();
    for utxo in utxos.iter() {
        let txid =
            bitcoin::Txid::from_slice(utxo.clone().outpoint.expect("outpoint").txid.as_slice())
                .expect("valid txid");
        assert!(txids.contains(&txid));
    }

    // Lets get some new utxos
    let mut pegins = Pegins::new();

    // create NUM_UTXOS pegins
    for _ in 0..NUM_UTXOS {
        let eth_address = ethers::core::types::Address::random();
        pegins.eth_addresses.push(eth_address);
        pegins.txids.push(rand::random::<[u8; 32]>());
    }

    for input in 0..NUM_UTXOS {
        let eth_address = pegins.eth_addresses.get(input).cloned().unwrap();
        let pk = clients[0]
            .get_gateway_address(tonic::Request::new(client::GetGatewayAddressRequest {
                eth_address: hex_encode(eth_address),
            }))
            .await
            .map_err(Error::Request)?
            .into_inner();
        let btc_address =
            Address::from_str(&pk.gateway_address).expect("valid address").assume_checked();
        pegins.btc_addresses.push(btc_address);
    }
    // only notify one of the clients
    for input in 0..NUM_UTXOS {
        let txid = pegins.txids.get(input).cloned().unwrap();
        let eth_address = pegins.eth_addresses.get(input).cloned().unwrap();
        let btc_address = pegins.btc_addresses.get(input).cloned().unwrap();
        let _ = send_pegin_notification(
            &mut clients[0],
            btc_address.clone(),
            hex_encode(eth_address),
            txid,
            1, // vout
            100_000_000,
        )
        .await?;
    }

    let first_utxo_commitment = clients[0]
        .get_utxo_merkle_root(tonic::Request::new(client::Empty {}))
        .await
        .unwrap()
        .into_inner()
        .merkle_root;

    let mut hashset = HashSet::new();
    for c in clients[1..].iter_mut() {
        let utxo_set = c
            .get_utxo_merkle_root(tonic::Request::new(client::Empty {}))
            .await
            .unwrap()
            .into_inner()
            .merkle_root;
        hashset.insert(utxo_set);
    }
    // all the btc_servers should have the same merkel commitment to the utxo set
    assert_eq!(hashset.len(), 1);
    assert_ne!(first_utxo_commitment, hashset.iter().next().unwrap().to_owned());

    // Submit many pegins at the same time
    let mut pegins = Pegins::new();
    // create NUM_UTXOS pegins
    for _ in 0..NUM_UTXOS {
        let eth_address = ethers::core::types::Address::random();
        pegins.eth_addresses.push(eth_address);
        pegins.txids.push(rand::random::<[u8; 32]>());
        let pk = clients[0]
            .get_gateway_address(tonic::Request::new(client::GetGatewayAddressRequest {
                eth_address: hex_encode(eth_address),
            }))
            .await
            .map_err(Error::Request)?
            .into_inner();
        let btc_address =
            Address::from_str(&pk.gateway_address).expect("valid address").assume_checked();
        pegins.btc_addresses.push(btc_address);
    }

    for c in clients.iter_mut() {
        let _ = send_pegins_notifications(
            c,
            pegins.txids.iter().map(|a| a.to_vec()).collect(),
            pegins.eth_addresses.iter().map(hex::encode).collect(),
            pegins.btc_addresses.clone(),
            vec![100_000_000; NUM_UTXOS],
        )
        .await?;
    }
    // get all utxos
    let mut all_utxos = clients[0]
        .get_all_utxos(tonic::Request::new(client::Empty {}))
        .await
        .unwrap()
        .into_inner()
        .utxos;

    // Filter out only for utxos from pegin
    all_utxos.retain(|utxo| {
        let txid = utxo.clone().outpoint.expect("outpoint").txid.to_vec();
        pegins.txids.iter().any(|peg_txid| txid == peg_txid)
    });
    it_info_print!("All utxos: {:?}", all_utxos);
    assert_eq!(all_utxos.len(), NUM_UTXOS);

    Ok(())
}
