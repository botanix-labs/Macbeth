use std::{collections::HashSet, str::FromStr};

use super::{
    error::Error,
    test_dkg::{do_dkg, send_pegin_notification},
};
use crate::suite::consensus::ConsensusIntegrationTestSuite;
use bitcoin::Address;
use client::BtcServerClient;
use hex::{self, encode as hex_encode};
use tonic::transport::Channel;

const NUM_UTXOS: usize = 10;

struct Pegins {
    pub eth_addresses: Vec<ethers::core::types::Address>,
    pub btc_addresses: Vec<Address>,
    pub txids: Vec<[u8; 32]>,
}

impl Pegins {
    fn new() -> Self {
        Pegins { eth_addresses: Vec::new(), btc_addresses: Vec::new(), txids: Vec::new() }
    }
}

pub async fn test_utxo_commitment(suite: &ConsensusIntegrationTestSuite) -> Result<(), Error> {
    // create pegins container
    let mut pegins = Pegins::new();

    // create NUM_UTXOS pegins
    for _ in 0..NUM_UTXOS {
        let eth_address = ethers::core::types::Address::random();
        pegins.eth_addresses.push(eth_address);
        pegins.txids.push(rand::random::<[u8; 32]>());
    }

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
            let _ = send_pegin_notification(c, btc_address.clone(), hex_encode(eth_address), txid)
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

    Ok(())
}
