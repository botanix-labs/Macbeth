
use std::{process::Stdio, str::FromStr, vec};

use tokio::{
    io::{self, AsyncBufReadExt},
    process::Command,
};

use bitcoin::{consensus::Encodable, Amount, FeeRate, TxOut};
use client;
use tonic::transport::Channel;

const NETWORK: bitcoin::Network = bitcoin::Network::Signet;
const _FEE_RATE: FeeRate = FeeRate::from_sat_per_vb_unchecked(30);

async fn spawn_server(id: u16, address: String) -> () {
    let working_directory = std::env::current_dir().unwrap();

    let identifier = id.to_string();
    let db_name = format!("db_{}", id);

    let command = "cargo";
    let args = vec![
        "run",
        "--",
        "--network",
        "testnet",
        "--db",
        db_name.as_str(),
        "--identifier",
        identifier.as_str(),
        "--address",
        address.as_str(),
    ];

    // Create a Command instance and set the working directory
    let mut cmd = Command::new(command);
    cmd.args(&args).current_dir(working_directory).stdout(Stdio::piped());

    // Spawn the command and handle its output
    let mut child = cmd.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();

    let mut lines = io::BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await.unwrap() {
        println!("** BTC SERVER ** >>> {:?}", line);
    }
}

fn clean_db(max: u16) {
    for i in 0..max {
        let db_name = format!("db_{}", i);
        std::fs::remove_dir_all(db_name).unwrap();
    }
}

fn spawn_n_servers(n: u16) -> Vec<tokio::task::JoinHandle<()>> {
    let mut tasks = vec![];
    for i in 0..n {
        let task = tokio::spawn(spawn_server(i, format!("0.0.0.0:{}", 8080 + i)));
        tasks.push(task);
    }
    tasks
}

async fn do_dkg(clients: &mut Vec<client::BtcServerClient<Channel>>) {
    // Round 1 dkg
    let mut round1_packages = vec![];
    for c in clients.iter_mut() {
        let p = c
            .get_round1_dkg_package(tonic::Request::new(client::Empty {}))
            .await
            .unwrap()
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
                .unwrap();
            }
        }
    }
    // Round 2 dkg
    let mut round2_packages = vec![];
    for c in clients.iter_mut() {
        let p = c
            .get_round2_dkg_package(tonic::Request::new(client::Empty {}))
            .await
            .unwrap()
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
                .unwrap();
            }
        }
    }
}

async fn send_pegin_notification(
    _secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::All>,
    client: &mut client::BtcServerClient<Channel>,
    eth_address: String,
    pk: String,
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
    let txid = rand::random::<[u8; 32]>();
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

#[tokio::test]
pub async fn dkg_flow() {
    let _secp = bitcoin::secp256k1::Secp256k1::new();
    let eth_1 = "86Bb524A1c7703C02BcEc36D1C4218aADb7D643D".to_string();
    let tasks = spawn_n_servers(3);

    // let servers come up
    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
    let mut c1 = client::BtcServerClient::connect("http://localhost:8080").await.unwrap();
    let mut c2 = client::BtcServerClient::connect("http://localhost:8081").await.unwrap();
    let mut c3 = client::BtcServerClient::connect("http://localhost:8082").await.unwrap();

    let mut clients = vec![c1.clone(), c2.clone(), c3.clone()];

    // Getting public key should fail
    let pk = c1
        .get_public_key(tonic::Request::new(client::GetPublicKeyRequest {
            eth_address: eth_1.clone(),
        }))
        .await;
    assert!(pk.is_err());
    let err = pk.err().unwrap();
    assert_eq!(err.code(), tonic::Code::Internal);
    assert_eq!(
        err.message(),
        "Failed to get public key: Missing key package, need to perform DKG first"
    );

    do_dkg(&mut clients).await;
    // After dkg we should be able to the dkg
    //// Get the pubkey
    let pk_1 = c1
        .get_public_key(tonic::Request::new(client::GetPublicKeyRequest {
            eth_address: eth_1.clone(),
        }))
        .await
        .unwrap()
        .into_inner();
    let pk_2 = c2
        .get_public_key(tonic::Request::new(client::GetPublicKeyRequest {
            eth_address: eth_1.clone(),
        }))
        .await
        .unwrap()
        .into_inner();
    let pk_3 = c3
        .get_public_key(tonic::Request::new(client::GetPublicKeyRequest {
            eth_address: eth_1.clone(),
        }))
        .await
        .unwrap()
        .into_inner();

    // Everyone got the same pks
    assert_eq!(pk_1.publickey, pk_2.publickey);
    assert_eq!(pk_1.publickey, pk_3.publickey);
    assert_eq!(pk_2.publickey, pk_3.publickey);

    // Ensure all pks can be serialized as secp public keys
    let _ = bitcoin::secp256k1::PublicKey::from_str(&pk_1.publickey).unwrap();
    let _ = bitcoin::secp256k1::PublicKey::from_str(&pk_2.publickey).unwrap();
    let _ = bitcoin::secp256k1::PublicKey::from_str(&pk_3.publickey).unwrap();

    // Test clean up
    for task in tasks {
        task.abort();
    }
    // Remove db dirs
    clean_db(3);
}

#[tokio::test]
async fn test_one_input_signing() {
    let _secp = bitcoin::secp256k1::Secp256k1::new();
    let signing_session_id = [0u8; 32];
    let eth_1 = "86Bb524A1c7703C02BcEc36D1C4218aADb7D643D".to_string();
    let tasks = spawn_n_servers(3);

    // let servers come up
    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
    let mut c1 = client::BtcServerClient::connect("http://localhost:8080").await.unwrap();
    let mut c2 = client::BtcServerClient::connect("http://localhost:8081").await.unwrap();
    let mut c3 = client::BtcServerClient::connect("http://localhost:8082").await.unwrap();

    let mut clients = vec![c1.clone(), c2.clone(), c3.clone()];

    do_dkg(&mut clients).await;
    // get the aggregate pk from any of the clients
    let pk = c1
        .get_public_key(tonic::Request::new(client::GetPublicKeyRequest {
            eth_address: eth_1.clone(),
        }))
        .await
        .unwrap()
        .into_inner();

    // Round 1 signing
    let c1_signing1 = c1
        .get_round1_signing_package(tonic::Request::new(client::Round1SigningPackageRequest {
            number_of_inputs: 1,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .unwrap()
        .into_inner();

    let c2_signing1 = c2
        .get_round1_signing_package(tonic::Request::new(client::Round1SigningPackageRequest {
            number_of_inputs: 1,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .unwrap()
        .into_inner();

    c3.new_round1_signing_package(tonic::Request::new(c1_signing1)).await.unwrap();
    c3.new_round1_signing_package(tonic::Request::new(c2_signing1)).await.unwrap();

    // Notify peg in
    let address1 = reth_btc_wallet::address::gateway_address(
        &bitcoin::secp256k1::PublicKey::from_str(&pk.publickey).unwrap(),
        NETWORK,
    )
    .unwrap();
    let prev_out1 =
        TxOut { script_pubkey: address1.script_pubkey(), value: Amount::from_sat(1000).to_sat() };

    let mut prev_out_bytes1 = Vec::new();
    prev_out1.consensus_encode(&mut prev_out_bytes1).unwrap();

    c3.notify_pegin(tonic::Request::new(client::NotifyPeginRequest {
        utxo_txid: "96e7187b4fb26f2d5c1699235ce1702dc373c009063da45aadca41bd85a866f6".to_string(),
        utxo_vout: 1,
        eth_address: eth_1.clone(),
        output: prev_out_bytes1,
    }))
    .await
    .unwrap();

    // Get signing package
    let signing_package = c3
        .get_to_sign_package(tonic::Request::new(client::ToSignRequest {
            fee_rate: 2,
            outputs: vec![client::Output {
                address: "mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh".to_string(),
                value: 800,
            }],
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .unwrap()
        .into_inner();

    // Round 2 signing
    let c1_signing2 = c1
        .get_round2_signing_package(tonic::Request::new(client::SignPayload {
            payload: signing_package.clone().payload,
            psbt: signing_package.clone().psbt,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .unwrap()
        .into_inner();

    let c2_signing2 = c2
        .get_round2_signing_package(tonic::Request::new(client::SignPayload {
            payload: signing_package.clone().payload,
            psbt: signing_package.clone().psbt,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .unwrap()
        .into_inner();

    c3.new_round2_signing_package(tonic::Request::new(c1_signing2)).await.unwrap();
    c3.new_round2_signing_package(tonic::Request::new(c2_signing2)).await.unwrap();

    let psbt = signing_package.clone().psbt;
    let finalized = c3
        .finalize_signing(tonic::Request::new(client::FinalizeSigningRequest {
            psbt,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .unwrap();

    println!("finalized: {:?}", finalized);

    // Test clean up
    for task in tasks {
        task.abort();
    }
    // Remove db dirs
    clean_db(3);
}

#[tokio::test]
async fn test_many_inputs_signing() {
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let eth_1 = "86Bb524A1c7703C02BcEc36D1C4218aADb7D643D".to_string();
    let eth_2 = "3C44CdDdB6a900fa2b585dd299e03d12FA4293BC".to_string();
    let signing_session_id = [0u8; 32];

    let tasks = spawn_n_servers(3);

    // let servers come up
    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
    let mut c1 = client::BtcServerClient::connect("http://localhost:8080").await.unwrap();
    let mut c2 = client::BtcServerClient::connect("http://localhost:8081").await.unwrap();
    let mut c3 = client::BtcServerClient::connect("http://localhost:8082").await.unwrap();

    let mut clients = vec![c1.clone(), c2.clone(), c3.clone()];

    do_dkg(&mut clients).await;
    // get the aggregate pk from any of the clients
    let pk1 = c1
        .get_public_key(tonic::Request::new(client::GetPublicKeyRequest {
            eth_address: eth_1.clone(),
        }))
        .await
        .unwrap()
        .into_inner();

    let pk2 = c1
        .get_public_key(tonic::Request::new(client::GetPublicKeyRequest {
            eth_address: eth_2.clone(),
        }))
        .await
        .unwrap()
        .into_inner();

    // Round 1 signing
    let c1_signing1 = c1
        .get_round1_signing_package(tonic::Request::new(client::Round1SigningPackageRequest {
            number_of_inputs: 2,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .unwrap()
        .into_inner();

    let c2_signing1 = c2
        .get_round1_signing_package(tonic::Request::new(client::Round1SigningPackageRequest {
            number_of_inputs: 2,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .unwrap()
        .into_inner();

    c3.new_round1_signing_package(tonic::Request::new(c1_signing1)).await.unwrap();
    c3.new_round1_signing_package(tonic::Request::new(c2_signing1)).await.unwrap();

    // Notify peg ins
    send_pegin_notification(&secp, &mut c3, eth_1.clone(), pk1.publickey.clone()).await;
    send_pegin_notification(&secp, &mut c3, eth_2.clone(), pk2.publickey.clone()).await;

    // Get signing package
    let signing_package = c3
        .get_to_sign_package(tonic::Request::new(client::ToSignRequest {
            fee_rate: 2,
            outputs: vec![client::Output {
                address: "mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh".to_string(),
                // Make sure to provide a value great than the value of one pegin (1000 sats)
                value: 1200,
            }],
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .unwrap()
        .into_inner();

    // Round 2 signing
    let c1_signing2 = c1
        .get_round2_signing_package(tonic::Request::new(client::SignPayload {
            payload: signing_package.clone().payload,
            psbt: signing_package.clone().psbt,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .unwrap()
        .into_inner();

    let c2_signing2 = c2
        .get_round2_signing_package(tonic::Request::new(client::SignPayload {
            payload: signing_package.clone().payload,
            psbt: signing_package.clone().psbt,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .unwrap()
        .into_inner();

    c3.new_round2_signing_package(tonic::Request::new(c1_signing2)).await.unwrap();
    c3.new_round2_signing_package(tonic::Request::new(c2_signing2)).await.unwrap();

    let psbt = signing_package.clone().psbt;
    let _finalized = c3
        .finalize_signing(tonic::Request::new(client::FinalizeSigningRequest {
            psbt,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .unwrap();

    // Test clean up
    for task in tasks {
        task.abort();
    }
    // Remove db dirs
    clean_db(3);
}
