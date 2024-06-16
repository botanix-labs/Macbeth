use std::str::FromStr;

use bitcoin::Address;
use bitcoincore_rpc::RpcApi;
use client::{BtcServerClient, SigningPackage, SigningPackageRequest};
use hex::{self, encode as hex_encode};
use tonic::transport::Channel;

use crate::suite::consensus::{
    frost::{
        error::Error,
        test_dkg::{do_dkg, send_pegin_notification},
    },
    ConsensusIntegrationTestSuite,
};

const INPUTS_TO_SPEND: usize = 2;

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

pub async fn test_many_inputs_signing(suite: &ConsensusIntegrationTestSuite) -> Result<(), Error> {
    let bitcoind = suite.global_context.bitcoind_rpc();
    let address = bitcoind.get_new_address(None, None).unwrap().assume_checked();
    // generate a block to the network looks live
    bitcoind.generate_to_address(1, &address).expect("generate to address");

    // create pegins container
    let mut pegins = Pegins::new();

    // create INPUTS_TO_SPEND pegins
    for _ in 0..INPUTS_TO_SPEND {
        let eth_address = ethers::core::types::Address::random();
        pegins.eth_addresses.push(eth_address);
        pegins.txids.push(rand::random::<[u8; 32]>());
    }

    // create a signing session id
    let signing_session_id = [0u8; 32];

    // create btc server clients
    let mut clients: Vec<BtcServerClient<Channel>> = vec![];
    for instance in 0..suite.global_context.instances {
        let port = suite
            .local_context
            .btc_servers
            .as_ref()
            .and_then(|servers| servers.iter().nth(instance as usize).map(|val| val.port))
            .ok_or_else(|| Error::InvalidBtcServerPort)?;
        let c = client::BtcServerClient::connect(format!("http://localhost:{port}"))
            .await
            .map_err(Error::ServerConnect)?;
        clients.push(c);
    }

    // Getting public key should fail for all clients
    for client in &mut clients {
        let pk = client.get_public_key(tonic::Request::new(client::Empty {})).await;
        assert!(pk.is_err());
        let err = pk.err().unwrap();
        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err.message().contains("missing key package"));
    }

    // run the dkg
    do_dkg(&mut clients).await?;

    // let say coordinator is account 0
    let coordinator_index: usize = clients.len() - 1;
    let mut coordinator = clients.get(coordinator_index).cloned().unwrap();

    // get the aggregate pk from any of the clients
    // Here we are signing for a INPUTS_TO_SPEND inputs that are tweaked differently
    for input in 0..INPUTS_TO_SPEND {
        let eth_address = pegins.eth_addresses.get(input).copied().unwrap();
        let pk = coordinator
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
        for input in 0..INPUTS_TO_SPEND {
            let txid = pegins.txids.get(input).copied().unwrap();
            let eth_address = pegins.eth_addresses.get(input).copied().unwrap();
            let btc_address = pegins.btc_addresses.get(input).cloned().unwrap();
            send_pegin_notification(c, btc_address.clone(), hex_encode(eth_address), txid).await?;
        }
    }

    // First step: get the PSBT
    let original_psbt = coordinator
        .get_psbt(tonic::Request::new(client::MakeTxRequest {
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
    // Signers will add their signing commitments to the psbt (including the coordinator)
    let mut round1_signing_commitments: Vec<SigningPackage> = vec![];
    for (index, client) in clients.iter_mut().enumerate() {
        // skip the coordinator here
        if coordinator_index == index {
            continue;
        }
        let c_signing = client
            .get_round1_signing_package(tonic::Request::new(client::SigningPackageRequest {
                psbt: original_psbt.clone(),
                signing_session_id: signing_session_id.to_vec(),
            }))
            .await
            .map_err(Error::Request)?
            .into_inner();
        round1_signing_commitments.push(c_signing);
    }

    // Coordinating node will collect the PSBTs with the signing commitments
    for signing_package in round1_signing_commitments {
        coordinator
            .new_round1_signing_package(tonic::Request::new(signing_package))
            .await
            .map_err(Error::Request)?;
    }

    // Signing Round 2
    // Get signing package
    let signing_package = coordinator
        .get_to_sign_package(tonic::Request::new(client::ToSignRequest {
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    // Signers should add their partial sigs to the psbt for each input
    let mut round2_signing_commitments: Vec<SigningPackage> = vec![];
    for (index, client) in clients.iter_mut().enumerate() {
        // skip the coordinator here
        if coordinator_index == index {
            continue;
        }
        let c_signing2 = client
            .get_round2_signing_package(tonic::Request::new(SigningPackageRequest {
                psbt: signing_package.clone().psbt,
                signing_session_id: signing_session_id.to_vec(),
            }))
            .await
            .map_err(Error::Request)?
            .into_inner();
        round2_signing_commitments.push(c_signing2);
    }

    // Coordinating node will collect the PSBTs with the partial sigs
    for signing_package in round2_signing_commitments {
        coordinator
            .new_round2_signing_package(tonic::Request::new(signing_package))
            .await
            .map_err(Error::Request)?;
    }

    let finalized = coordinator
        .finalize_signing(tonic::Request::new(client::FinalizeSigningRequest {
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await;

    assert!(finalized.is_err());
    let err = finalized.err().unwrap();
    assert_eq!(err.code(), tonic::Code::Internal);
    // Test should fail here after `psbt.finalize_mut()`.
    // These inputs don't actually exist on the regtest chain so there is nothing to be spent.
    // In the future we can generate some addresses send funds and use those outpoints for this
    // test.
    assert!(
        err.message().contains("bad-txns-inputs-missingorspent") ||
            err.message().contains("Missing inputs")
    );

    /*
    // Lets try spending again, this time should be spending non tweaked inputs (change)
    // First lets get a new signing session id
    let signing_session_id = [1u8; 32];
    // First step: get the PSBT
    let original_psbt = coordinator
        .get_psbt(tonic::Request::new(client::MakeTxRequest {
            outputs: vec![client::Output {
                address: "mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh".to_string(),
                // At this point there should be 200 sats in the wallet
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
    let mut round1_signing_commitments: Vec<SigningPackage> = vec![];
    for (_, client) in clients.iter_mut().enumerate() {
        let c_signing = client
            .get_round1_signing_package(tonic::Request::new(client::SigningPackageRequest {
                psbt: original_psbt.clone(),
                signing_session_id: signing_session_id.to_vec(),
            }))
            .await
            .map_err(Error::Request)?
            .into_inner();
        round1_signing_commitments.push(c_signing);
    }

    // Coordinating node will collect the PSBTs with the signing commitments
    for signing_package in round1_signing_commitments.into_iter() {
        coordinator
            .new_round1_signing_package(tonic::Request::new(signing_package))
            .await
            .map_err(Error::Request)?;
    }

    // Get signing package
    let signing_package = coordinator
        .get_to_sign_package(tonic::Request::new(client::ToSignRequest {
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?
        .into_inner();

    // Signers should add their partial sigs to the psbt for each input
    let mut round2_signing_commitments: Vec<SigningPackage> = vec![];
    for (_, client) in clients.iter_mut().enumerate() {
        let c_signing2 = client
            .get_round2_signing_package(tonic::Request::new(client::SignPayload {
                psbt: signing_package.clone().psbt,
                signing_session_id: signing_session_id.to_vec(),
            }))
            .await
            .map_err(Error::Request)?
            .into_inner();
        round2_signing_commitments.push(c_signing2);
    }

    // Coordinating node will collect the PSBTs with the partial sigs
    for signing_package in round2_signing_commitments.into_iter() {
        coordinator
            .new_round2_signing_package(tonic::Request::new(signing_package))
            .await
            .map_err(Error::Request)?;
    }

    let _finalized = coordinator
        .finalize_signing(tonic::Request::new(client::FinalizeSigningRequest {
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(Error::Request)?;
    */

    Ok(())
}
