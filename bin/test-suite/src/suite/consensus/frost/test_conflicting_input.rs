use std::str::FromStr;

use crate::{
    suite::consensus::frost::test_signing::{do_signing, Pegin},
    utils::{
        get_checkpoint_block_hash, send_pegin_notification, send_pegout_notification,
        MIN_BLOCKS_COINBASE_MATURE,
    },
};
use bitcoin::{consensus::Encodable, Address};
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
    utils::generate_blocks,
};

const NUM_PEGINS: usize = 5;

pub async fn test_conflicting_input(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), anyhow::Error> {
    let bitcoind = suite.global_context.bitcoind_rpc();
    // Load up the bitcoin wallet and generate some blocks
    for wallet in bitcoind.list_wallets()? {
        it_info_print!("#UNLOADING WALLET?", &wallet);
        let _ = bitcoind.unload_wallet(Some(&wallet))?;
    }
    let wallet_name = get_unique_wallet_name();
    let create_res = bitcoind.create_wallet(&wallet_name, None, None, None, None);
    if create_res.is_err() {
        tracing::info!("Wallet already exists, loading wallet ...");
        // wallet already exists, load wallet
        let _ = bitcoind.load_wallet(&wallet_name);
    }
    generate_blocks(&bitcoind, MIN_BLOCKS_COINBASE_MATURE).await;

    // create pegins container
    let mut pegins = vec![];

    // create btc server clients
    let mut clients = suite
        .local_context
        .btc_server_clients
        .clone()
        .expect("btc server rpc clients to be defined");

    // run the dkg
    do_dkg(&mut clients).await?;
    let amount_to_send = bitcoin::Amount::from_sat(100_000);
    // create NUM_PEGINS pegins
    for _ in 0..NUM_PEGINS {
        let eth_address = ethers::core::types::Address::random();
        // Lets get the gateway address for this eth address
        let mut client =
            clients.get(0).cloned().ok_or_else(|| anyhow::anyhow!("client not found"))?;
        let res = client
            .get_gateway_address(tonic::Request::new(btc_server_client::GetGatewayAddressRequest {
                eth_address: hex_encode(eth_address),
            }))
            .await
            .map_err(Error::Request)?
            .into_inner();
        let btc_address = Address::from_str(&res.gateway_address)?.assume_checked();
        let txid = bitcoind.send_to_address(
            &btc_address,
            amount_to_send,
            None,
            None,
            None,
            None,
            None,
            None,
        )?;

        // Generate some block to confirm it
        generate_blocks(&bitcoind, 2).await;

        let tx_res = bitcoind.get_transaction(&txid, None)?;
        let pegin_tx = tx_res.transaction()?;
        let spk = btc_address.script_pubkey();
        let (vout, pegin_output) = pegin_tx
            .output
            .iter()
            .enumerate()
            .find(|(_, o)| o.script_pubkey == spk)
            .ok_or_else(|| anyhow::anyhow!("pegin output not found"))?;

        pegins.push(Pegin {
            eth_address,
            btc_address,
            outpoint: bitcoin::OutPoint { txid, vout: vout as u32 },
            amount: pegin_output.value,
        });
    }

    // get the checkpoint blockhash
    let bitcoind = suite.global_context.bitcoind_rpc();
    let checkpoint_block_hash = get_checkpoint_block_hash(&bitcoind)?;

    // get the aggregate pk from any of the clients
    // Here we are signing for a NUM_PEGINS inputs that are tweaked differently

    // Notify peg ins to all peers
    // signers will not sign if they cannot locate the UTXOs they are being requested to sign
    for c in clients.iter_mut() {
        for pegin in pegins.iter() {
            let ot = pegin.outpoint;
            let mut txid_bytes = Vec::with_capacity(32);
            ot.txid.consensus_encode(&mut txid_bytes)?;
            send_pegin_notification(
                c,
                checkpoint_block_hash.clone(),
                pegin.btc_address.clone(),
                hex_encode(pegin.eth_address),
                txid_bytes.try_into().map_err(|_| anyhow::anyhow!("invalid txid"))?,
                ot.vout,
                pegin.amount.to_sat(),
            )
            .await?;
        }
    }

    // Using stdRng here as it implements Send
    let mut rand = StdRng::from_entropy();
    let mut pegout_id_bytes = [0u8; 36];
    rand.fill_bytes(&mut pegout_id_bytes);
    let pegout_id =
        PegoutId::from_bytes(&pegout_id_bytes).map_err(|_| anyhow::anyhow!("invalid pegout id"))?;

    let secp = bitcoin::secp256k1::Secp256k1::new();
    let sk = bitcoin::PrivateKey::generate(bitcoin::Network::Regtest);
    let pk = sk.public_key(&secp);
    let spk = pk.p2wpkh_script_code().expect("valid pk");

    // Notify some pending pegouts
    let amount = bitcoin::Amount::from_sat(100_000);
    for c in clients.iter_mut() {
        // Each pegin is 100_000 satoshis, spending 100_000 should spend at least 2 inputs
        send_pegout_notification(
            c,
            checkpoint_block_hash.clone(),
            amount.to_sat(),
            1,
            pegout_id,
            spk.clone(),
        )
        .await?;
    }
    // signs, broadcasts, and tracks the psbt honoring pending pegouts
    let _tracked_tx = do_signing(&mut clients, &bitcoind, &[1u8; 32]).await?;

    // resend the same pegout notification so it is a pending pegout
    for c in clients.iter_mut() {
        send_pegout_notification(
            c,
            checkpoint_block_hash.clone(),
            amount.to_sat(),
            1,
            pegout_id,
            spk.clone(),
        )
        .await?;
    }

    // Create a new psbt that honors the pending pegout that is already being tracked in the
    // previous psbt.

    // FAILURE CASES:
    // 1) Signers will reject the psbt if it contains no conflicting input. This does not happen b/c
    //    the coordinator will include a conflicting input and signers will validate against this
    // 2) The network will reject the broadcast psbt if it contains a conflicting input with error
    //    "txn-mempool-conflict" since the first tx is still in the mempool. This won't happen as we
    //    will signal for RBF in the pegout inputs.
    // 3) The network will reject the broadcast psbt if the replacement fee isn't high enough.  We
    //    are not trying to replace the tx so this is an acceptable failure case as well:
    //    "insufficient fee, rejecting replacement"
    // We want failure case 3.
    if let Err(e) = do_signing(&mut clients, &bitcoind, &[2u8; 32]).await {
        let error_message = e.to_string();
        assert!(
            error_message.contains("insufficient fee, rejecting replacement"),
            "Unexpected error: {}",
            error_message
        );
        return Ok(());
    }

    Err(anyhow::anyhow!("expected txn-mempool-conflict error"))
}
