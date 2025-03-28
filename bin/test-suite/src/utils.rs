use crate::{
    it_info_print,
    suite::consensus::{
        common::{botanix_client::BotanixEthClient, events::GatewayAddressResponse},
        frost::error::Error,
    },
};
use bitcoin::{consensus::Encodable, hash_types::BlockHash, Address, Amount};
use bitcoincore_rpc::RpcApi;
use btcserverlib::pegout_id::PegoutId;
use ethers::providers::{JsonRpcClient, Provider, ProviderError};
use reth_chainspec::BOTANIX_TESTNET;
use reth_primitives::Address as EthAddress;
use serde::Deserialize;
use std::time::Duration;
use tokio::time::sleep;
use tonic::transport::Channel;

pub const MIN_BLOCKS_COINBASE_MATURE: u32 = 101;

/// Generate `num_blocks` blocks on the given bitcoind instance
pub async fn generate_blocks(bitcoind: &impl RpcApi, num_blocks: u32) -> Vec<BlockHash> {
    let address = bitcoind.get_new_address(None, None).unwrap().assume_checked();
    let mut block_hashes = vec![];
    for _ in 0..num_blocks {
        // You could generate many blocks at once here but occasionally
        // We get a `SocketError`
        match bitcoind.generate_to_address(1, &address) {
            Ok(hashes) => {
                block_hashes.push(hashes);
            }
            Err(e) => {
                it_info_print!("Error generating blocks: {:?}", e);
                panic!("generate to address failed");
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    block_hashes.into_iter().flatten().collect::<Vec<_>>()
}

// Uses random spk and pegout id
pub async fn send_pegout_notification(
    client: &mut client::BtcServerClient<Channel>,
    checkpoint_block_hash: Vec<u8>,
    amount: u64,
    bitcoin_height: u64,
    pegout_id: PegoutId,
    spk: bitcoin::ScriptBuf,
) -> Result<(), Error> {
    let pending_pegouts = vec![client::PendingPegout {
        pegout_id: pegout_id.clone().as_bytes().to_vec(),
        spk: spk.to_bytes().to_vec(),
        amount,
        height: bitcoin_height,
    }];

    client
        .new_consensus_checkpoint(client::ConsensusCheckpointRequest {
            checkpoint_block_hash,
            pegins: vec![],
            pending_pegouts,
        })
        .await
        .map_err(|e| {
            it_info_print!("Error: {:?}", e);
            Error::ConsensusCheckpoint
        })?;

    Ok(())
}

pub async fn send_pegin_notification(
    client: &mut client::BtcServerClient<Channel>,
    checkpoint_block_hash: Vec<u8>,
    address: Address,
    eth_address: String,
    txid: [u8; 32],
    vout: u32,
    amount: u64,
) -> Result<(), Error> {
    let mut prev_out_bytes = Vec::new();
    address.script_pubkey().consensus_encode(&mut prev_out_bytes).unwrap();
    let utxos = [client::Utxo {
        output: Some(client::TxOut {
            value: Amount::from_sat(amount).to_sat(),
            script_pubkey: Some(client::ScriptBuf { script: prev_out_bytes }),
        }),
        outpoint: Some(client::OutPoint { txid: txid.to_vec(), vout }),
        eth_address,
    }]
    .to_vec();

    client
        .new_consensus_checkpoint(client::ConsensusCheckpointRequest {
            checkpoint_block_hash,
            pegins: utxos,
            pending_pegouts: vec![],
        })
        .await
        .map_err(|e| {
            it_info_print!("Error: {:?}", e);
            Error::ConsensusCheckpoint
        })?;

    Ok(())
}

pub async fn send_pegins_notifications(
    client: &mut client::BtcServerClient<Channel>,
    checkpoint_block_hash: Vec<u8>,
    txids: Vec<Vec<u8>>,
    eth_addresses: Vec<String>,
    btc_addresses: Vec<Address>,
    amounts: Vec<u64>,
) -> Result<(), Error> {
    assert_eq!(txids.len(), eth_addresses.len());
    assert_eq!(txids.len(), btc_addresses.len());
    assert_eq!(txids.len(), amounts.len());

    let mut utxos = Vec::new();
    for (i, txid) in txids.iter().enumerate() {
        let eth_address = eth_addresses[i].clone();
        let btc_address = btc_addresses[i].clone();
        let amount = amounts[i];

        let mut prev_out_bytes = Vec::new();
        btc_address.script_pubkey().consensus_encode(&mut prev_out_bytes).unwrap();
        utxos.push(client::Utxo {
            output: Some(client::TxOut {
                value: Amount::from_sat(amount).to_sat(),
                script_pubkey: Some(client::ScriptBuf { script: prev_out_bytes }),
            }),
            outpoint: Some(client::OutPoint { txid: txid.to_vec(), vout: 1 }),
            eth_address,
        });
    }

    client
        .new_consensus_checkpoint(client::ConsensusCheckpointRequest {
            checkpoint_block_hash,
            pegins: utxos,
            pending_pegouts: vec![],
        })
        .await
        .map_err(|e| {
            it_info_print!("Error: {:?}", e);
            Error::ConsensusCheckpoint
        })?;

    Ok(())
}

// Need to define a custom struct as breaking changes in bitcoin-core will cause the
// deserialization to fail
#[derive(Deserialize)]
pub struct BlockChainInfoRes {
    pub blocks: u64,
}

pub fn get_checkpoint_block_hash(bitcoind: &impl RpcApi) -> Result<Vec<u8>, Error> {
    let deep_tip = bitcoind.call::<BlockChainInfoRes>("getblockchaininfo", &[]).unwrap().blocks -
        (BOTANIX_TESTNET.parent_confirmation_depth as u64);
    let deep_block_hash = bitcoind.get_block_hash(deep_tip).unwrap();
    let mut checkpoint_block_hash = vec![];
    if let Err(e) = deep_block_hash.consensus_encode(&mut checkpoint_block_hash) {
        it_info_print!("Error: {:?}", e);
        return Err(Error::ConsensusEncode);
    };

    Ok(checkpoint_block_hash)
}

// TODO: determine root cause for this call sometimes failing and remove the retry logic
pub async fn get_gateway_address_with_retry<P>(
    provider: Provider<P>,
    destination: EthAddress,
    max_retries: u32,
) -> anyhow::Result<GatewayAddressResponse, Error>
where
    P: JsonRpcClient,
{
    let mut gateway_address_response: Result<GatewayAddressResponse, ProviderError> =
        Err(ProviderError::UnsupportedRPC);
    for attempt in 0..max_retries {
        gateway_address_response = provider
            .request::<Vec<String>, GatewayAddressResponse>(
                "eth_getGatewayAddress",
                vec![hex::encode(destination.0)],
            )
            .await;

        match &gateway_address_response {
            Ok(_) => {
                break;
            }
            Err(e) => {
                it_info_print!("gatewayaddress call failed: {:?}", e);

                if attempt < max_retries - 1 {
                    sleep(Duration::from_millis((500 * (attempt + 1)).into())).await;
                }
            }
        }
    }

    gateway_address_response.map_err(|_| Error::GatewayAddressNotAvailable)
}

/// Waits until the genesis block exists.
pub async fn wait_until_genesis_block_exists(client: &BotanixEthClient) -> Result<(), Error> {
    while client
        .get_latest_block()
        .await
        .map_err(|_| Error::LatestBlockDoesNotExist)?
        .number
        .ok_or(Error::LatestBlockDoesNotExist)? ==
        0.into()
    {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Ok(())
}
