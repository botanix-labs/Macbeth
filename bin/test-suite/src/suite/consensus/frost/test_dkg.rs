use super::error::Error;
use crate::suite::consensus::ConsensusIntegrationTestSuite;

use btcserverlib::frost_id;
use client::{self, BtcServerClient};
use frost_secp256k1_tr as frost;
use std::{collections::BTreeMap, str::FromStr, vec};
use tonic::transport::Channel;

pub async fn dkg_flow(suite: &ConsensusIntegrationTestSuite) -> Result<(), Error> {
    // create btc server clients
    let mut clients: Vec<BtcServerClient<Channel>> = vec![];
    for instance in 0..suite.global_context.fed_instances {
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
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("Missing key package"));
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
    // creating a mapping of client index to fronst identifier
    let mut frost_id_map = BTreeMap::new();
    for (i, _) in clients.iter().enumerate() {
        let frost_id = frost_id!(i as u16);
        frost_id_map.insert(frost_id, i);
    }
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
    // Not much to assert here, we can check the length of the ids
    for p in round2_packages.iter() {
        if p.identifier.len() != 32 {
            return Err(Error::Round2PackagesLenghtMismatch);
        }
    }

    // Send dkg round2 shares to each recipient
    for (i, p) in round2_packages.iter().enumerate() {
        let from_frost_id = frost_id!(i as u16).serialize().to_vec();
        let round2_shares = p.payload.clone();
        let shares = serde_json::from_slice::<
            BTreeMap<frost::Identifier, frost::keys::dkg::round2::Package>,
        >(&round2_shares)
        .expect("Failed to deserialize round 2 shares");

        // there should always be n-1 shares
        assert_eq!(shares.len(), clients.len() - 1);
        for (_j, (identifier, payload)) in shares.iter().enumerate() {
            let client_index = frost_id_map.get(&identifier).unwrap();
            let mut client = clients[*client_index].clone();
            client
                .new_round2_dkg_package(tonic::Request::new(client::DkgPayload {
                    identifier: from_frost_id.clone(),
                    payload: serde_json::to_vec(&payload).unwrap(),
                }))
                .await
                .map_err(Error::Request)?;
        }
    }
    Ok(())
}
