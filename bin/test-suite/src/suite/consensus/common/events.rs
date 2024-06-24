use crate::{
    it_info_print,
    suite::consensus::common::poa_node::{is_dkg_ready, FederationMemberTestConfig, Notifications},
};
use client::SigningStatus;
use reth_primitives::{Receipt, B256};
use reth_provider::chain::BlockReceipts;
use std::collections::HashMap;

pub const BITCOIND_WALLET_NAME: &str = "botanix_integration_test_wallet";
pub const SEND_AMOUNT: u64 = 1; // = 1 ether

pub async fn await_dkg(
    fed_members: &mut HashMap<u16, FederationMemberTestConfig>,
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
            let block_receipts = canon_state_notification.notification.block_receipts();
            let non_reverted_block_receipts = block_receipts
                .into_iter()
                .filter_map(|(receipt, reverted)| if !reverted { Some(receipt) } else { None })
                .collect::<Vec<BlockReceipts>>();
            let final_block_receipts =
                non_reverted_block_receipts.into_iter().fold(vec![], |mut acc, receipts| {
                    let receipts = receipts
                        .tx_receipts
                        .into_iter()
                        .filter_map(|(_, r)| if r.success { Some(r) } else { None })
                        .collect::<Vec<Receipt>>();
                    acc.extend(receipts);
                    acc
                });
            it_info_print!("Final block receipts", final_block_receipts.len());
            for block_receipt in final_block_receipts.into_iter() {
                for log in block_receipt.logs.into_iter() {
                    for topic in log.topics() {
                        if *topic == event_topic {
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
