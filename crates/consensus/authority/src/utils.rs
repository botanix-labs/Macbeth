use client::{BtcServerClient, MakeTxRequest, NotifyPeginRequest};
use reth_botanix_lib::mint_validation::{
    parse_pegin_reth_log_topic, parse_pegout_reth_log_topic, GenesisContractEvents,
};
use reth_btc_wallet::block_source::{BlockSource, MempoolSpace};

use reth_primitives::{hex, Log};
use reth_provider::BundleStateWithReceipts;

use tracing::{debug, error, info};

/// Repersents an error while processing a botanix log
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessBotanixLogError {
    /// Failed to notify btc server about pegin
    #[error("Failed to notify btc server about pegin")]
    FailedToNotifyPegin(tonic::Status),
    #[error("Failed to broadcast pegout tx")]
    FailedToBroadcastPegout,
    #[error("Failed to make pegout tx")]
    FailedToMakePegoutTx(tonic::Status),
}

// TODO(armins) ideally processing these reciepts dont have sideeffects or make network calls
// in the future the caller should be responsible for doing this 

/// Processes the receipts in the given `bundle_state` and performs actions based on the receipt
/// logs.
///
/// This function iterates over the receipts in the bundle and for each receipt, it checks if it is
/// a prunning block or if it is successful. If the receipt is successful, it processes each log in
/// the receipt and calls the `process_botanix_log` function. Finally, it logs the receipt
/// information.
///
/// # Arguments
///
/// * `bundle_state` - The bundle state with receipts to process.
/// * `should_broadcast_pegout` - A boolean indicating whether to broadcast pegout or not.
///
/// # Returns
///
/// Returns `Ok(())` if the processing is successful, otherwise returns an error of type
/// `ProcessBotanixLogError`.
pub(crate) async fn process_reciepts(
    bitcoin_block_source: &MempoolSpace,
    btc_server: &mut BtcServerClient<tonic::transport::Channel>,
    bundle_state: &BundleStateWithReceipts,
    should_broadcast_pegout: bool,
) -> Result<(), ProcessBotanixLogError> {
    let reciepts_bundle = bundle_state.receipts().iter();
    for (index, reciepts) in reciepts_bundle.enumerate() {
        for reciept in reciepts {
            if index == 0 && reciept.is_none() {
                // Prunning block, skip
                break
            }
            if let Some(reciept) = reciept {
                if !reciept.success {
                    continue
                }
                for log in &reciept.logs {
                    process_botanix_log(
                        bitcoin_block_source,
                        btc_server,
                        log,
                        should_broadcast_pegout,
                    )
                    .await?;
                }
            }
            info!(target: "consensus::authority", "Reciept {:?}", reciept);
        }
    }
    Ok(())
}

/// Processes a single botanix log and performs actions based on the log's topics.
///
/// This function checks the topics of the log and performs different actions based on the topic.
/// If the topic is `GenesisContractEvents::MintingEvent`, it parses and sends the minting event to
/// the `btc_server`. If the topic is `GenesisContractEvents::BurnEvent` and
/// `should_broadcast_pegout` is true, it parses and sends the withdrawal event to the `btc_server`.
///
/// # Arguments
///
/// * `log` - The log to process.
/// * `should_broadcast_pegout` - A boolean indicating whether to broadcast pegout or not.
///
/// # Returns
///
/// Returns `Ok(())` if the processing is successful, otherwise returns an error of type
/// `ProcessBotanixLogError`.

async fn process_botanix_log(
    bitcoin_block_source: &MempoolSpace,
    btc_server: &mut BtcServerClient<tonic::transport::Channel>,
    log: &Log,
    should_broadcast_pegout: bool,
) -> Result<(), ProcessBotanixLogError> {
    for topic in &log.topics {
        match GenesisContractEvents::try_from(topic.clone()) {
            Ok(GenesisContractEvents::MintingEvent) => {
                info!(target: "consensus::authority", "Parsing and sending minting event to btc_server");
                let pegin_data = parse_pegin_reth_log_topic(&log)
                    .expect("passed evm check should pass this parse attempt");
                for pegin in &pegin_data.meta {
                    let request = NotifyPeginRequest {
                        utxo_txid: pegin.outpoint.txid.to_string(),
                        utxo_vout: pegin.outpoint.vout,
                        eth_address: hex::encode(pegin.address.to_vec()),
                        output: bitcoin::consensus::serialize(
                            pegin.tx.output.get(pegin.outpoint.vout as usize).expect("valid vout"),
                        ),
                    };
                    btc_server
                        .notify_pegin(request)
                        .await
                        .map_err(|e| ProcessBotanixLogError::FailedToNotifyPegin(e))?;
                    info!(target: "consensus::authority", "notifying btc server about pegin utxo");
                }
            }
            Ok(GenesisContractEvents::BurnEvent) => {
                if !should_broadcast_pegout {
                    continue;
                }
                // TODO (armins): obv
                let fee_rate = 30u32;
                info!(target: "consensus::authority", "Parsing and sending withdrawal event to btc_server");
                let pegout = parse_pegout_reth_log_topic(&log).expect("valid pegout request");
                let request = MakeTxRequest {
                    address: pegout.destination.to_string(),
                    value: pegout.amount.to_sat(),
                    fee_rate,
                };

                let response = btc_server
                    .make_tx(request)
                    .await
                    .map_err(|e| ProcessBotanixLogError::FailedToMakePegoutTx(e))?;

                let raw_tx = response.into_inner().tx;
                info!(target: "consensus::authority", "broadcasting withdrawal tx");

                bitcoin_block_source
                    .broadcast_tx(&hex::encode(raw_tx))
                    .await
                    .map_err(|_| ProcessBotanixLogError::FailedToBroadcastPegout)?;
            }
            Err(e) => {
                debug!(target: "consensus::authority", ?e, "Non-genesis contract event");
                continue
            }
        }
    }
    Ok(())
}
