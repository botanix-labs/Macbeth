use std::{str::FromStr, time::Duration};

use bitcoin::Address;
use bitcoincore_rpc::RpcApi;
use btcserverlib::pegout_id::PegoutId;
use hex::{self, encode as hex_encode};
use rand::{rngs::StdRng, RngCore, SeedableRng};

use crate::{
    it_info_print,
    suite::consensus::{
        common::events::get_unique_wallet_name,
        frost::{error::Error, test_dkg::do_dkg},
        ConsensusIntegrationTestSuite,
    },
    utils::{
        generate_blocks, get_checkpoint_block_hash, send_pegin_notification,
        send_pegout_notification,
    },
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

pub async fn test_pending_pegouts(suite: &ConsensusIntegrationTestSuite) -> Result<(), Error> {
    let bitcoind = suite.global_context.bitcoind_rpc();
    // Load up the bitcoin wallet and generate some blocks
    for wallet in bitcoind.list_wallets().unwrap() {
        it_info_print!("#UNLOADING WALLET?", &wallet);
        let _ = bitcoind.unload_wallet(Some(&wallet));
    }
    let wallet_name = get_unique_wallet_name();
    let create_res = bitcoind.create_wallet(&wallet_name, None, None, None, None);
    if create_res.is_err() {
        // wallet already exists, load wallet
        let _ = bitcoind.load_wallet(&wallet_name);
    }
    // generate a block to the network looks live
    generate_blocks(&bitcoind, 1).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // create pegins container
    let mut pegins = Pegins::new();

    // create INPUTS_TO_SPEND pegins
    for _ in 0..INPUTS_TO_SPEND {
        let eth_address = ethers::core::types::Address::random();
        pegins.eth_addresses.push(eth_address);
        pegins.txids.push(rand::random::<[u8; 32]>());
    }

    // create btc server clients
    let mut clients = suite
        .local_context
        .btc_server_clients
        .clone()
        .expect("btc server rpc clients to be defined");

    // Getting public key should fail for all clients
    for client in &mut clients {
        let pk = client.get_public_key(tonic::Request::new(client::Empty {})).await;
        assert!(pk.is_err());
        let err = pk.err().unwrap();
        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err.message().contains("Missing key package"));
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

    // get the checkpoint blockhash
    let bitcoind = suite.global_context.bitcoind_rpc();
    let checkpoint_block_hash = get_checkpoint_block_hash(&bitcoind)?;

    // Notify peg ins to all peers
    // signers will not sign if they cannot locate the UTXOs they are being requested to sign
    for c in clients.iter_mut() {
        for input in 0..INPUTS_TO_SPEND {
            let txid = pegins.txids.get(input).copied().unwrap();
            let eth_address = pegins.eth_addresses.get(input).copied().unwrap();
            let btc_address = pegins.btc_addresses.get(input).cloned().unwrap();
            send_pegin_notification(
                c,
                checkpoint_block_hash.clone(),
                btc_address.clone(),
                hex_encode(eth_address),
                txid,
                0,           // vout
                100_000_000, // Amount
            )
            .await?;
        }
    }

    let mut pending_pegouts_sent: Vec<(bitcoin::ScriptBuf, PegoutId, u64, u64)> = vec![];
    let secp = bitcoin::secp256k1::Secp256k1::new();
    // Send 50 pegout notifications
    for _ in 0..50 {
        // generate random amount and height
        let amount = rand::random::<u64>() % 1_000_000;
        let height = rand::random::<u64>() % 100_000;
        // Using stdRng here as it implements Send
        let mut rand = StdRng::from_entropy();
        let mut pegout_id_bytes = [0u8; 36];
        rand.fill_bytes(&mut pegout_id_bytes);
        let pegout_id = PegoutId::from_bytes(&pegout_id_bytes).unwrap();

        let pk = bitcoin::PrivateKey::generate(bitcoin::Network::Regtest).public_key(&secp);
        let spk = pk.p2wpkh_script_code().expect("valid pk");

        send_pegout_notification(
            &mut clients[0],
            checkpoint_block_hash.clone(),
            amount,
            height,
            pegout_id,
            spk.clone(),
        )
        .await?;
        pending_pegouts_sent.push((spk, pegout_id, amount, height));
    }

    let pending_pegouts = clients[0]
        .get_pending_pegouts(tonic::Request::new(client::Empty {}))
        .await
        .expect("get pending pegouts")
        .into_inner();
    println!("pending_pegouts: {:?}", pending_pegouts);
    let mut pending_pegouts = pending_pegouts.pending_pegouts.clone();
    pending_pegouts.dedup();

    assert_eq!(pending_pegouts.len(), pending_pegouts_sent.len());
    // Check that all pending pegouts are in the response
    for pegout in pending_pegouts {
        let pegout_id = PegoutId::from_bytes(&pegout.pegout_id).unwrap();
        let spk = bitcoin::ScriptBuf::from_bytes(pegout.spk);
        let amount = pegout.amount;
        let height = pegout.height;
        assert!(pending_pegouts_sent.contains(&(spk, pegout_id, amount, height)));
    }
    // TODO here we can spend some of the pending pegouts and ensure they are removed from the
    // pending pegouts list

    Ok(())
}
