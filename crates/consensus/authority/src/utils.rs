use client::{BtcServerClient, MakeTxRequest, NotifyPeginRequest};
use futures_util::{stream::FuturesUnordered, StreamExt};
use reth_botanix_lib::mint_validation::{
    parse_pegin_reth_log_topic, parse_pegout_reth_log_topic, GenesisContractEvents, BURN_TOPIC,
    MINT_CONTRACT_ADDRESS, MINT_TOPIC,
};
use reth_btc_wallet::bitcoind::BitcoindClient;

use reth_primitives::{constants::eip225::EPOCH_LENGTH, hex, Bloom, BloomInput, Log, Receipt};
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

/// Search a receipt for a pegout and return a MakeTxRequest for the pegout
///
/// # Arguments
///
/// * `receipt` - The receipt to search for a pegout.
///
/// # Returns
///
/// Returns `Some(MakeTxRequest)` if a pegout is found in the receipt, otherwise returns `None`.
pub(crate) async fn make_tx_request_for_pegout_in_receipt(
    receipt: Receipt,
) -> Option<MakeTxRequest> {
    if !receipt.success {
        info!(target: "consensus::authority", "Receipt status code is not success {:?}", receipt);
        return None;
    }

    let mut futures = Vec::new();
    for log in receipt.logs {
        futures.push(make_tx_request_for_pegout(log));
    }

    let mut results_stream = futures.into_iter().map(tokio::spawn).collect::<FuturesUnordered<_>>();
    while let Some(pegout) = results_stream.next().await {
        match pegout {
            Ok(Some(pegout)) => return Some(pegout),
            Ok(None) => continue,
            Err(e) => {
                error!(target: "consensus::authority", ?e, "Error fetching pegout");
                return None;
            }
        }
    }
    None
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
pub(crate) async fn process_receipts(
    bitcoind_client: &BitcoindClient,
    btc_server: &mut BtcServerClient<tonic::transport::Channel>,
    bundle_state: &BundleStateWithReceipts,
    should_broadcast_pegout: bool,
) -> Result<(), ProcessBotanixLogError> {
    let receipts_bundle = bundle_state.receipts().iter();
    for (index, receipts) in receipts_bundle.enumerate() {
        for receipt in receipts {
            if index == 0 && receipt.is_none() {
                // Prunning block, skip
                break;
            }
            if let Some(receipt) = receipt {
                if !receipt.success {
                    continue;
                }
                for log in &receipt.logs {
                    process_botanix_log(bitcoind_client, btc_server, log, should_broadcast_pegout)
                        .await?;
                }
            }
            info!(target: "consensus::authority", "Receipt {:?}", receipt);
        }
    }
    Ok(())
}

/// Search a log for a pegout and return a MakeTxRequest for the pegout
///
/// # Arguments
///
/// * `log` - The log to search for a pegout.
///
/// # Returns
///
/// Returns `Some(MakeTxRequest)` if a pegout is found in the log, otherwise returns `None`.
async fn make_tx_request_for_pegout(log: Log) -> Option<MakeTxRequest> {
    for topic in &log.topics {
        match GenesisContractEvents::try_from(*topic) {
            Ok(GenesisContractEvents::MintingEvent) => continue,
            Ok(GenesisContractEvents::BurnEvent) => {
                let fee_rate = 30u32;
                let pegout = parse_pegout_reth_log_topic(&log).expect("valid pegout request");
                return Some(MakeTxRequest {
                    address: pegout.destination.to_string(),
                    value: pegout.amount.to_sat(),
                    fee_rate,
                });
            }
            Err(e) => {
                debug!(target: "consensus::authority", ?e, "Non burn event");
                continue;
            }
        }
    }
    None
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

// TODO (scott) remove `should_broadcast_pegout` since this only happens for epoch block
// check if pegout and store in cache
// create util method to send pegouts
async fn process_botanix_log(
    bitcoind_client: &BitcoindClient,
    btc_server: &mut BtcServerClient<tonic::transport::Channel>,
    log: &Log,
    should_broadcast_pegout: bool,
) -> Result<(), ProcessBotanixLogError> {
    for topic in &log.topics {
        match GenesisContractEvents::try_from(*topic) {
            Ok(GenesisContractEvents::MintingEvent) => {
                info!(target: "consensus::authority", "Parsing and sending minting event to btc_server");
                let pegin_data = parse_pegin_reth_log_topic(log)
                    .expect("passed evm check should pass this parse attempt");
                for pegin in &pegin_data.meta {
                    let request = NotifyPeginRequest {
                        utxo_txid: pegin.outpoint.txid.to_string(),
                        utxo_vout: pegin.outpoint.vout,
                        eth_address: hex::encode(pegin.address),
                        output: bitcoin::consensus::serialize(
                            pegin.tx.output.get(pegin.outpoint.vout as usize).expect("valid vout"),
                        ),
                    };
                    btc_server
                        .notify_pegin(request)
                        .await
                        .map_err(ProcessBotanixLogError::FailedToNotifyPegin)?;
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
                let pegout = parse_pegout_reth_log_topic(log).expect("valid pegout request");
                let request = MakeTxRequest {
                    address: pegout.destination.to_string(),
                    value: pegout.amount.to_sat(),
                    fee_rate,
                };

                let response = btc_server
                    .make_tx(request)
                    .await
                    .map_err(ProcessBotanixLogError::FailedToMakePegoutTx)?;

                let raw_tx = response.into_inner().tx;
                info!(target: "consensus::authority", "broadcasting withdrawal tx");

                bitcoind_client
                    .broadcast_tx(&hex::encode(raw_tx))
                    .await
                    .map_err(|_| ProcessBotanixLogError::FailedToBroadcastPegout)?;
            }
            Err(e) => {
                debug!(target: "consensus::authority", ?e, "Non-genesis contract event");
                continue;
            }
        }
    }
    Ok(())
}

fn bloom_contains_minting_contract_address(bloom: Bloom) -> bool {
    bloom.contains_input(BloomInput::Raw(MINT_CONTRACT_ADDRESS.as_ref()))
}

pub(crate) fn bloom_contains_pegout(bloom: Bloom) -> bool {
    bloom_contains_minting_contract_address(bloom) &&
        bloom.contains_input(BloomInput::Raw(BURN_TOPIC.as_ref()))
}

pub(crate) fn bloom_contains_pegin(bloom: Bloom) -> bool {
    bloom_contains_minting_contract_address(bloom) &&
        bloom.contains_input(BloomInput::Raw(MINT_TOPIC.as_ref()))
}

/// Returns true if the given block number is the end of an epoch
/// by checking if the next block is the start of a new epoch
///
/// # Arguments
///
/// * `current_block_number` - The current block number
///
/// # Returns
///
/// Returns `true` if the given block number is the end of an epoch, otherwise returns `false`.
pub(crate) fn is_epoch_end(current_block_number: u64) -> bool {
    (current_block_number + 1) % EPOCH_LENGTH == 0
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use reth_primitives::{address, b256, bloom, bytes, Header, B256, U256};

    use super::*;

    #[test]
    fn test_bloom_contains_pegout() {
        let mut bloom = Bloom::default();
        assert!(!bloom_contains_pegout(bloom));

        // add minting contract address to bloom
        bloom.accrue(BloomInput::Raw(MINT_CONTRACT_ADDRESS.as_ref()));

        // assert still false
        assert!(!bloom_contains_pegout(bloom));

        // add minting burn topic to bloom
        bloom.accrue(BloomInput::Raw(BURN_TOPIC.as_ref()));

        // assert true
        assert!(bloom_contains_pegout(bloom))
    }

    struct MockBlock {
        header: Header,
    }

    impl MockBlock {
        fn new(logs_bloom: Bloom) -> Self {
            MockBlock {
                header: Header {
                    parent_hash: B256::from_str(
                        "13a7ec98912f917b3e804654e37c9866092043c13eb8eab94eb64818e886cff5",
                    )
                    .unwrap(),
                    ommers_hash: b256!(
                        "1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"
                    ),
                    beneficiary: address!("f97e180c050e5ab072211ad2c213eb5aee4df134"),
                    state_root: b256!(
                        "ec229dbe85b0d3643ad0f471e6ec1a36bbc87deffbbd970762d22a53b35d068a"
                    ),
                    transactions_root: b256!(
                        "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"
                    ),
                    receipts_root: b256!(
                        "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"
                    ),
                    logs_bloom,
                    difficulty: U256::from(0),
                    number: 0x30598,
                    gas_limit: 0x1c9c380,
                    gas_used: 0,
                    timestamp: 0x64c40d54,
                    extra_data: bytes!("d883010c01846765746888676f312e32302e35856c696e7578"),
                    mix_hash: b256!(
                        "70ccadc40b16e2094954b1064749cc6fbac783c1712f1b271a8aac3eda2f2325"
                    ),
                    nonce: 0,
                    base_fee_per_gas: Some(7),
                    withdrawals_root: Some(b256!(
                        "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"
                    )),
                    parent_beacon_block_root: None,
                    blob_gas_used: Some(0),
                    excess_blob_gas: Some(0x1600000),
                },
            }
        }
    }

    // this test is redundant to the test above but better illustrates the use case
    #[test]
    fn test_bloom_contains_pegout_in_header() {
        // bloom filter that contains the minting contract address and burn topic
        let mut bloom = Bloom::default();
        bloom.accrue(BloomInput::Raw(MINT_CONTRACT_ADDRESS.as_ref()));
        bloom.accrue(BloomInput::Raw(BURN_TOPIC.as_ref()));

        let block = MockBlock::new(bloom);

        assert!(bloom_contains_pegout(block.header.logs_bloom));
    }

    #[test]
    fn test_bloom_contains_pegin() {
        let mut bloom = Bloom::default();
        assert!(!bloom_contains_pegin(bloom));

        // add minting contract address to bloom
        bloom.accrue(BloomInput::Raw(MINT_CONTRACT_ADDRESS.as_ref()));

        // assert still false
        assert!(!bloom_contains_pegin(bloom));

        // add minting mint topic to bloom
        bloom.accrue(BloomInput::Raw(MINT_TOPIC.as_ref()));

        // assert true
        assert!(bloom_contains_pegin(bloom))
    }

    #[test]
    fn test_is_epoch_end() {
        let start_block_1 = 0;
        let end_block_1 = EPOCH_LENGTH - 1;
        let start_block_2 = end_block_1 + 1;
        let end_block_2 = start_block_2 + EPOCH_LENGTH - 1;
        let start_block_3 = end_block_2 + 1;

        assert!(!is_epoch_end(start_block_1));
        assert!(is_epoch_end(end_block_1));
        assert!(!is_epoch_end(start_block_2));
        assert!(is_epoch_end(end_block_2));
        assert!(!is_epoch_end(start_block_3));
    }
}
