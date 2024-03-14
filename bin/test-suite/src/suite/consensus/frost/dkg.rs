use super::error::Error;
use crate::suite::consensus::{
    frost::{
        btc_server::{clean_db, spawn_n_btc_servers},
        do_dkg,
    },
    ConsensusIntegrationTestSuite,
};
use std::str::FromStr;

pub async fn dkg_flow(suite: &ConsensusIntegrationTestSuite) -> Result<(), Error> {
    let _secp = bitcoin::secp256k1::Secp256k1::new();
    let eth_1 = "86Bb524A1c7703C02BcEc36D1C4218aADb7D643D".to_string();
    let mut tasks = spawn_n_btc_servers(3);

    // let servers come up
    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

    // create clients
    let port =
        tasks.iter().nth(0).map(|val| val.port).ok_or_else(|| Error::InvalidBtcServerPort)?;
    println!("PORT 1 {:?}", port);
    let mut c1 = client::BtcServerClient::connect(format!("http://localhost:{}", port))
        .await
        .map_err(Error::ServerConnect)?;
    let port =
        tasks.iter().nth(1).map(|val| val.port).ok_or_else(|| Error::InvalidBtcServerPort)?;
    println!("PORT 2 {:?}", port);
    let mut c2 = client::BtcServerClient::connect(format!("http://localhost:{}", port))
        .await
        .map_err(Error::ServerConnect)?;
    let port =
        tasks.iter().nth(2).map(|val| val.port).ok_or_else(|| Error::InvalidBtcServerPort)?;
    println!("PORT 3 {:?}", port);
    let mut c3 = client::BtcServerClient::connect(format!("http://localhost:{}", port))
        .await
        .map_err(Error::ServerConnect)?;
    let mut clients = vec![c1.clone(), c2.clone(), c3.clone()];

    // Getting public key should fail
    let pk = c1
        .get_public_key(tonic::Request::new(client::GetPublicKeyRequest {
            eth_address: eth_1.clone(),
        }))
        .await;
    println!("XXXXXXXXXXXXXXXXXXXXXXXX {:?}", pk);
    assert!(pk.is_err());
    let err = pk.err().unwrap();
    assert_eq!(err.code(), tonic::Code::Internal);
    assert_eq!(err.message(), "Failed to get public key: missing key package");
    let _ = do_dkg(&mut clients).await?;
    // After dkg we should be able to the dkg
    //// Get the pubkey
    let pk_1 = c1
        .get_public_key(tonic::Request::new(client::GetPublicKeyRequest {
            eth_address: eth_1.clone(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();
    let pk_2 = c2
        .get_public_key(tonic::Request::new(client::GetPublicKeyRequest {
            eth_address: eth_1.clone(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();
    let pk_3 = c3
        .get_public_key(tonic::Request::new(client::GetPublicKeyRequest {
            eth_address: eth_1.clone(),
        }))
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

    // Test clean up
    for task in tasks.iter_mut() {
        let _ = task.child_process.kill().await;
    }
    // Remove db dirs
    clean_db(&tasks);

    Ok(())
}
