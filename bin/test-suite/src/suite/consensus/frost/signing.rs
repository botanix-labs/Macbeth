use super::{
    btc_server::{clean_db, spawn_n_btc_servers},
    do_dkg,
    error::Error,
    send_pegin_notification,
};
use crate::suite::consensus::ConsensusIntegrationTestSuite;

pub async fn test_many_inputs_signing(suite: &ConsensusIntegrationTestSuite) -> Result<(), Error> {
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let eth_1 = "86Bb524A1c7703C02BcEc36D1C4218aADb7D643D".to_string();
    let eth_2 = "3C44CdDdB6a900fa2b585dd299e03d12FA4293BC".to_string();
    let signing_session_id = [0u8; 32];

    let mut tasks = spawn_n_btc_servers(3);

    // let servers come up
    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

    // create clients
    let port =
        tasks.iter().nth(0).map(|val| val.port).ok_or_else(|| Error::InvalidBtcServerPort)?;
    let mut c1 = client::BtcServerClient::connect(format!("http://localhost:{}", port))
        .await
        .map_err(Error::ServerConnect)?;
    let port =
        tasks.iter().nth(1).map(|val| val.port).ok_or_else(|| Error::InvalidBtcServerPort)?;
    let mut c2 = client::BtcServerClient::connect(format!("http://localhost:{}", port))
        .await
        .map_err(Error::ServerConnect)?;
    let port =
        tasks.iter().nth(2).map(|val| val.port).ok_or_else(|| Error::InvalidBtcServerPort)?;
    let mut c3 = client::BtcServerClient::connect(format!("http://localhost:{}", port))
        .await
        .map_err(Error::ServerConnect)?;

    let mut clients = vec![c1.clone(), c2.clone(), c3.clone()];

    let _ = do_dkg(&mut clients).await?;
    // get the aggregate pk from any of the clients
    // Here we are signing for a two inputs that are tweaked differently
    let pk1 = c1
        .get_public_key(tonic::Request::new(client::GetPublicKeyRequest {
            eth_address: eth_1.clone(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    let pk2 = c1
        .get_public_key(tonic::Request::new(client::GetPublicKeyRequest {
            eth_address: eth_2.clone(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    let txid1 = rand::random::<[u8; 32]>();
    let txid2 = rand::random::<[u8; 32]>();
    // Notify peg ins to all peers
    // signers will not sign if they cannot locate the UTXOs they are being requested to sign
    send_pegin_notification(&secp, &mut c1, eth_1.clone(), pk1.publickey.clone(), txid1).await;
    send_pegin_notification(&secp, &mut c1, eth_2.clone(), pk2.publickey.clone(), txid2).await;

    send_pegin_notification(&secp, &mut c2, eth_1.clone(), pk1.publickey.clone(), txid1).await;
    send_pegin_notification(&secp, &mut c2, eth_2.clone(), pk2.publickey.clone(), txid2).await;

    send_pegin_notification(&secp, &mut c3, eth_1.clone(), pk1.publickey.clone(), txid1).await;
    send_pegin_notification(&secp, &mut c3, eth_2.clone(), pk2.publickey.clone(), txid2).await;

    // First step: get the PSBT
    let original_psbt = c3
        .get_psbt(tonic::Request::new(client::MakeTxRequest {
            fee_rate: 2,
            outputs: vec![client::Output {
                address: "mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh".to_string(),
                // At this point there should be 2000 sats in the wallet
                value: 1200,
            }],
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner()
        .psbt;

    // Round 1 signing
    // Signers will add their signing commitments to the psbt
    let c1_signing1 = c1
        .get_round1_signing_package(tonic::Request::new(client::Round1SigningPackageRequest {
            psbt: original_psbt.clone(),
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    let c2_signing1 = c2
        .get_round1_signing_package(tonic::Request::new(client::Round1SigningPackageRequest {
            psbt: original_psbt.clone(),
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    // Coordinating node will collect the PSBTs with the signing commitments
    c3.new_round1_signing_package(tonic::Request::new(c1_signing1))
        .await
        .map_err(Error::Request)?;
    c3.new_round1_signing_package(tonic::Request::new(c2_signing1))
        .await
        .map_err(Error::Request)?;

    // Signing Round 2
    // Get signing package
    let signing_package = c3
        .get_to_sign_package(tonic::Request::new(client::ToSignRequest {
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    // Signers should add their partial sigs to the psbt for each input
    let c1_signing2 = c1
        .get_round2_signing_package(tonic::Request::new(client::SignPayload {
            psbt: signing_package.clone().psbt,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    let c2_signing2 = c2
        .get_round2_signing_package(tonic::Request::new(client::SignPayload {
            psbt: signing_package.clone().psbt,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    // Coordinating node will collect the PSBTs with the partial sigs
    c3.new_round2_signing_package(tonic::Request::new(c1_signing2))
        .await
        .map_err(Error::Request)?;
    c3.new_round2_signing_package(tonic::Request::new(c2_signing2))
        .await
        .map_err(Error::Request)?;

    let psbt = signing_package.clone().psbt;
    let _finalized = c3
        .finalize_signing(tonic::Request::new(client::FinalizeSigningRequest {
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?;

    // Lets try spending again, this time should be spending non tweaked inputs (change)
    // First lets get a new signing session id
    let signing_session_id = [1u8; 32];
    // First step: get the PSBT
    let original_psbt = c3
        .get_psbt(tonic::Request::new(client::MakeTxRequest {
            fee_rate: 2,
            outputs: vec![client::Output {
                address: "mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh".to_string(),
                // At this point there should be 800 sats in the wallet
                value: 200,
            }],
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner()
        .psbt;
    // Round 1 signing
    // Signers will add their signing commitments to the psbt
    let c1_signing1 = c1
        .get_round1_signing_package(tonic::Request::new(client::Round1SigningPackageRequest {
            psbt: original_psbt.clone(),
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    let c2_signing1 = c2
        .get_round1_signing_package(tonic::Request::new(client::Round1SigningPackageRequest {
            psbt: original_psbt.clone(),
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();
    // Coordinating node will collect the PSBTs with the signing commitments
    c3.new_round1_signing_package(tonic::Request::new(c1_signing1))
        .await
        .map_err(Error::Request)?;
    c3.new_round1_signing_package(tonic::Request::new(c2_signing1))
        .await
        .map_err(Error::Request)?;

    // Signing Round 2
    let signing_package = c3
        .get_to_sign_package(tonic::Request::new(client::ToSignRequest {
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    // Signers should add their partial sigs to the psbt for each input
    let c1_signing2 = c1
        .get_round2_signing_package(tonic::Request::new(client::SignPayload {
            psbt: signing_package.clone().psbt,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    let c2_signing2 = c2
        .get_round2_signing_package(tonic::Request::new(client::SignPayload {
            psbt: signing_package.clone().psbt,
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    c3.new_round2_signing_package(tonic::Request::new(c1_signing2))
        .await
        .map_err(Error::Request)?;
    c3.new_round2_signing_package(tonic::Request::new(c2_signing2))
        .await
        .map_err(Error::Request)?;
    let _finalized = c3
        .finalize_signing(tonic::Request::new(client::FinalizeSigningRequest {
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?;

    // Test clean up
    for task in tasks.iter_mut() {
        let _ = task.child_process.kill().await;
    }
    // Remove db dirs
    clean_db(&tasks);

    Ok(())
}
