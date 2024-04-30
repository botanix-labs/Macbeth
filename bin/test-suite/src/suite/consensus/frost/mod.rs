pub mod botanix_client;
pub mod btc_server;
pub mod error;
pub mod poa_node;

// tests
pub mod test_block_builder;
pub mod test_dkg;
pub mod test_frost_e2e;
pub mod test_frost_e2e_edge_cases;
pub mod test_signing;
pub mod test_utxo_commitment;

use crate::{it_info_print, suite::consensus::frost::poa_node::Notifications};
use reth_primitives::{Receipt, B256};
use reth_provider::chain::BlockReceipts;
use std::collections::HashMap;

use poa_node::{is_dkg_ready, FederationMemberTestConfig};

const BITCOIND_WALLET_NAME: &str = "botanix_integration_test_wallet";
const SEND_AMOUNT: u64 = 1; // = 1 ether

pub async fn await_dkg(
    fed_members: &mut HashMap<u16, FederationMemberTestConfig>,
    rx: &mut tokio::sync::mpsc::Receiver<Notifications>,
) {
    let mut pub_keys = vec![];
    while let Some(notification) = rx.recv().await {
        match notification {
            Notifications::DkgFinished(dkg_notification) => {
                if let Some(fed_member) = fed_members.get_mut(&dkg_notification.engine_index) {
                    fed_member.is_dkg_ready = true;
                    pub_keys.push(dkg_notification.public_key)
                }
                if is_dkg_ready(&fed_members) {
                    it_info_print!("FED MEMBERS DKG KEYS ------->", &pub_keys);
                    assert!(pub_keys.len() == fed_members.len());
                    pub_keys.dedup();
                    assert!(pub_keys.len() == 1);
                    break;
                }
            }
            _ => {}
        }
    }
}

pub async fn await_botanix_event(
    rx: &mut tokio::sync::mpsc::Receiver<Notifications>,
    event_topic: B256,
) {
    // wait for a few blocks to make sure the tx got included and mined
    while let Some(notification) = rx.recv().await {
        match notification {
            Notifications::CanonState(canon_state_notification) => {
                it_info_print!("Canon state notification", canon_state_notification);
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
                it_info_print!("Final block receipts", final_block_receipts);
                for block_receipt in final_block_receipts.into_iter() {
                    for log in block_receipt.logs.into_iter() {
                        for topic in log.topics.into_iter() {
                            if topic == event_topic {
                                return;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct GatewayAddressResponse {
    gateway_address: String,
    aggregate_public_key: String,
    eth_address: String,
}
