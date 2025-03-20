use crate::{
    it_info_print,
    suite::consensus::common::poa_node::{is_dkg_ready, FederationMemberTestConfig, Notifications},
};
use client::SigningStatus;
use reth_primitives::B256;
use std::collections::BTreeMap;

pub const BITCOIND_WALLET_NAME: &str = "botanix_integration_test_wallet";
pub const SEND_AMOUNT: u64 = 1; // = 1 ether

pub async fn await_dkg(
    fed_members: &mut BTreeMap<u16, FederationMemberTestConfig>,
    rx: &mut tokio::sync::broadcast::Receiver<Notifications>,
) {
    let mut pub_keys = vec![];
    it_info_print!("Awaiting DKG");
    while let Ok(notification) = rx.recv().await {
        if let Notifications::DkgFinished(dkg_notification) = notification {
            if let Some(fed_member) = fed_members.get_mut(&dkg_notification.engine_index) {
                fed_member.is_dkg_ready = true;
                pub_keys.push(dkg_notification.public_key)
            }
            if is_dkg_ready(&fed_members) {
                it_info_print!("Federation members public keys:", &pub_keys);
                assert!(pub_keys.len() == fed_members.len());
                pub_keys.dedup();
                assert!(pub_keys.len() == 1);
                break;
            }
        }
    }
}

#[allow(dead_code)]
pub async fn await_signing_completion(
    in_turn_member_index: u16,
    rx: &mut tokio::sync::broadcast::Receiver<Notifications>,
) {
    while let Ok(notification) = rx.recv().await {
        if let Notifications::SigningStatusReport((member_index, _session_id, status)) =
            notification
        {
            if in_turn_member_index == member_index && status.eq(&SigningStatus::Finalized) {
                break;
            }
        }
    }
}

pub async fn await_botanix_event(
    rx: &mut tokio::sync::broadcast::Receiver<Notifications>,
    event_topic: B256,
) {
    // wait for a few blocks to make sure the tx got included and mined
    while let Ok(notification) = rx.recv().await {
        if let Notifications::CanonState(canon_state_notification) = notification {
            it_info_print!(
                "Canon state notification for engine index =",
                canon_state_notification.engine_index
            );
            let block_receipts = canon_state_notification.tx_receipts;
            it_info_print!("Final block receipts", block_receipts.len());
            for block_receipt in block_receipts.into_iter() {
                for log in block_receipt.logs.into_iter() {
                    for topic in log.topics {
                        if topic.0 == event_topic.0 {
                            return;
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct GatewayAddressResponse {
    pub gateway_address: String,
    pub aggregate_public_key: String,
    pub eth_address: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ExtraDataHeader {
    pub aggregated_public_key: String,
    pub bitcoin_block_hash: String,
    pub block_producer_address: String,
    pub chain_version: u64,
    pub version: u64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockWithEDH {
    pub base_fee_per_gas: String,
    pub blob_gas_used: String,
    pub difficulty: String,
    pub excess_blob_gas: String,
    pub extra_data: String,
    pub extra_data_header: ExtraDataHeader,
    pub gas_limit: String,
    pub gas_used: String,
    pub hash: String,
    pub logs_bloom: String,
    pub miner: String,
    pub mix_hash: String,
    pub nonce: String,
    pub number: String,
    pub parent_beacon_block_root: String,
    pub parent_hash: String,
    pub receipts_root: String,
    pub sha3_uncles: String,
    pub size: String,
    pub state_root: String,
    pub timestamp: String,
    pub total_difficulty: String,
    pub transactions: Vec<String>,
    pub transactions_root: String,
    pub uncles: Vec<String>,
}
