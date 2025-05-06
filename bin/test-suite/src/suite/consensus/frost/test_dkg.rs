use super::error::Error;
use crate::suite::consensus::ConsensusIntegrationTestSuite;

use btcserverlib::frost_id;
use client::{self, BtcServerClient};
use frost_secp256k1_tr as frost;
use std::{
    collections::{BTreeMap, VecDeque},
    str::FromStr,
    vec,
};
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

    let mut queue: VecDeque<client::DkgPayload> = VecDeque::new();

    // Kick-off the initial DKG payloads; only the coordinator will have a
    // payloads to send.
    for client in clients.iter_mut() {
        let p = client
            .get_dkg_payloads(tonic::Request::new(client::Empty {}))
            .await
            .map_err(Error::Request)?
            .into_inner();

        for p in p.payloads {
            queue.push_back(p);
        }
    }

    // Forward each payload to the correct client, and push the resulting
    // payloads back into the queue.
    while let Some(p) = queue.pop_front() {
        // Find the corresponding client.
        let frost_id = frost::Identifier::deserialize(&p.recipient).unwrap();
        let idx = frost_id_map.get(&frost_id).unwrap();
        let recipient = clients.get_mut(*idx).unwrap();

        let p = recipient.new_dkg_payload(p).await.map_err(Error::Request)?.into_inner();

        for p in p.payloads {
            queue.push_back(p);
        }
    }

    // At this point, the DKG should be complete, and all clients should have
    // the same public key.

    Ok(())
}
