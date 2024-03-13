use bitcoin::{block::Header, psbt::PartiallySignedTransaction, witness::Witness};
use client::{BtcServerClient, MakeTxRequest, NotifyPeginRequest, Output};
use futures_util::{stream::FuturesUnordered, StreamExt};
use reth_botanix_lib::{
    mint_validation::{
        parse_pegin_reth_log_topic, parse_pegout_reth_log_topic, GenesisContractEvents, BURN_TOPIC,
        MINT_CONTRACT_ADDRESS, MINT_TOPIC,
    },
    peg_contract::PegoutData,
};
use reth_btc_wallet::bitcoind::BitcoindClient;

use reth_primitives::{
    constants::{
        eip225::EPOCH_LENGTH, MAINNET_PEGIN_CONFIRMATION_DEPTH, SIGNET_PEGIN_CONFIRMATION_DEPTH,
    },
    hex, Bloom, BloomInput, Log, Receipt, BOTANIX_TESTNET,
};
use reth_provider::BundleStateWithReceipts;

use tracing::{debug, error, info, warn};

/// Repersents an error while processing a botanix log
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessBotanixLogError {
    /// Failed to notify btc server about pegin
    #[error("Failed to notify btc server about pegin")]
    FailedToNotifyPegin(tonic::Status),
    #[error("Failed to broadcast pegout tx")]
    FailedToBroadcastPegout,
    #[error("Failed to make pegout tx: {0}")]
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
/// Returns `Some(PegoutData)` if a pegout is found in the receipt, otherwise returns `None`.
pub(crate) async fn make_tx_request_for_pegout_in_receipt(receipt: Receipt) -> Option<PegoutData> {
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
/// information and returns a list of pegins if they exist.
///
/// # Arguments
///
/// * `btc_server` - The btc server client.
/// * `bundle_state` - The bundle state with receipts to process.
/// * `recent_bitcoin_block_height` - The most recent known bitcoin block height.
/// * `is_testnet` - A boolean indicating whether the chain is a testnet or not.
///
/// # Returns
///
/// Returns `Ok(Vec<MakeTxRequest>)` if the processing is successful, otherwise returns an error of
/// type `ProcessBotanixLogError`.
pub(crate) async fn process_receipts(
    btc_server: &mut BtcServerClient<tonic::transport::Channel>,
    bundle_state: &BundleStateWithReceipts,
    recent_bitcoin_block_height: u32,
    is_testnet: bool,
) -> Result<Vec<MakeTxRequest>, ProcessBotanixLogError> {
    let mut pegouts: Vec<MakeTxRequest> = Vec::new();
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
                    match process_botanix_log(
                        btc_server,
                        log,
                        recent_bitcoin_block_height,
                        is_testnet,
                    )
                    .await
                    {
                        Ok(Some(pegout)) => {
                            pegouts.push(pegout);
                        }
                        Ok(None) => continue,
                        Err(e) => {
                            error!(target: "consensus::authority", ?e, "Failed to process botanix log");
                            return Err(e);
                        }
                    }
                }
            }
            info!(target: "consensus::authority", "Receipt {:?}", receipt);
        }
    }
    Ok(pegouts)
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
async fn make_tx_request_for_pegout(log: Log) -> Option<PegoutData> {
    for topic in &log.topics {
        match GenesisContractEvents::try_from(*topic) {
            Ok(GenesisContractEvents::MintingEvent) => continue,
            Ok(GenesisContractEvents::BurnEvent) => {
                return Some(parse_pegout_reth_log_topic(&log).expect("valid pegout request"));
            }
            Err(e) => {
                debug!(target: "consensus::authority", ?e, "Non burn event");
                continue;
            }
        }
    }
    None
}

pub(crate) async fn send_pegouts(
    bitcoin_block_source: &BitcoindClient,
    btc_server: &mut BtcServerClient<tonic::transport::Channel>,
    pegouts: Vec<PegoutData>,
) -> Result<(), ProcessBotanixLogError> {
    let req = MakeTxRequest {
        outputs: pegouts
            .iter()
            .map(|pegout| Output {
                address: pegout.destination.to_string(),
                value: pegout.amount.to_sat(),
            })
            .collect(),
        // TODO Pull from bitcoind
        fee_rate: 30u32,
        signing_session_id: [0u8; 32].to_vec(),
    };

    match btc_server.get_psbt(req).await {
        Ok(response) => {
            // TODO progress with FROST signing here
        }
        Err(e) => {
            error!(target: "consensus::authority", ?e, "Failed to make pegout tx");
            return Err(ProcessBotanixLogError::FailedToMakePegoutTx(e));
        }
    }

    Ok(())
}

/// Processes a single botanix log and performs actions based on the log's topics.
///
/// This function checks the topics of the log and performs different actions based on the topic.
/// If the topic is `GenesisContractEvents::MintingEvent`, it parses and sends the minting event to
/// the `btc_server`. If the topic is `GenesisContractEvents::BurnEvent`, it validates the pegout
/// and returns it.
///
/// # Arguments
///
/// * `btc_server` - The btc server client.
/// * `log` - The log to process.
/// * `recent_bitcoin_block_height` - The most recent known bitcoin block height.
/// * `is_testnet` - A boolean indicating whether the chain is a testnet or not.
///
/// # Returns
///
/// Returns `Ok(Option<MakeTxRequest>)` if the processing is successful, otherwise returns an error
/// of type `ProcessBotanixLogError`.
async fn process_botanix_log(
    btc_server: &mut BtcServerClient<tonic::transport::Channel>,
    log: &Log,
    recent_bitcoin_block_height: u32,
    is_testnet: bool,
) -> Result<Option<MakeTxRequest>, ProcessBotanixLogError> {
    let mut pegout: Option<MakeTxRequest> = None;
    for topic in &log.topics {
        match GenesisContractEvents::try_from(*topic) {
            Ok(GenesisContractEvents::MintingEvent) => {
                info!(target: "consensus::authority", "Parsing and sending minting event to btc_server");
                let pegin_data = parse_pegin_reth_log_topic(log)
                    .expect("passed evm check should pass this parse attempt");
                // enforce required confirmation depth by network
                let confirmation_depth = get_confirmation_depth(is_testnet);
                if pegin_data.bitcoin_block_height >
                    recent_bitcoin_block_height - confirmation_depth
                {
                    warn!(target: "consensus::authority", "pegin confirmation depth not met, skipping");
                    continue;
                }
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
                // TODO(scott): make dynamic
                let fee_rate = 30u32;

                // validate pegout
                info!(target: "consensus::authority", "Validating pegout");
                // TODO comment this back in when FROST signing is implemented
                // match parse_pegout_reth_log_topic(&log) {
                //     Ok(parsed_pegout) => {
                //         pegout = Some(MakeTxRequest {
                //             address: parsed_pegout.destination.to_string(),
                //             value: parsed_pegout.amount.to_sat(),
                //             fee_rate,
                //         });
                //     }
                //     Err(e) => {
                //         error!(target: "consensus::authority", ?e, "Failed to parse pegout");
                //         return Err(ProcessBotanixLogError::FailedToMakePegoutTx);
                //     }
                // }
            }
            Err(e) => {
                debug!(target: "consensus::authority", ?e, "Non-genesis contract event");
                continue;
            }
        }
    }
    Ok(pegout)
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

/// Finds the starting block number for the current epoch based on the current block number
///
/// # Arguments
///
/// * `epoch_length` - The length of an epoch
/// * `current_block_number` - The current block number
///
/// # Returns
///
/// Returns the starting block number for the current epoch.
pub(crate) fn find_epoch_start(epoch_length: u64, current_block_number: u64) -> u64 {
    let mut start_block_number = current_block_number;
    while start_block_number % epoch_length != 0 {
        start_block_number -= 1;
    }
    start_block_number
}

/// Returns the recent block height from the given recent bitcoin block header.
///
/// # Arguments
///
/// * `recent_bitcoin_block_header` - The recent bitcoin block header
///
/// # Returns
///
/// Returns the recent block height or 0 if None.
pub(crate) fn get_recent_block_height_or_zero(
    recent_bitcoin_block_header: Option<(Header, u32)>,
) -> u32 {
    recent_bitcoin_block_header.map(|(_, height)| height).unwrap_or_else(|| {
        error!(target: "consensus::authority", "Failed to get recent bitcoin block height");
        0
    })
}

pub(crate) fn get_confirmation_depth(is_testnet: bool) -> u32 {
    match is_testnet {
        true => SIGNET_PEGIN_CONFIRMATION_DEPTH,
        false => MAINNET_PEGIN_CONFIRMATION_DEPTH,
    }
}

pub(crate) fn is_testnet(chain_id: u64) -> bool {
    chain_id == BOTANIX_TESTNET.chain().id()
}

pub(crate) fn get_witness_data_from_psbt(psbt: PartiallySignedTransaction) -> Vec<Witness> {
    psbt.inputs.iter().filter_map(|input| input.final_script_witness.clone()).collect()
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bitcoin::{hash_types::TxMerkleNode, hashes::Hash, psbt::{Input, PartiallySignedTransaction}, BlockHash, CompactTarget, TxIn};
    use rand::Rng;
    use reth_primitives::{address, b256, bytes, Header, B256, U256};

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

    #[test]
    fn test_find_epoch_start() {
        let mut rng = rand::thread_rng();

        let current_block_1 = 0;
        let current_block_2 = current_block_1 + rng.gen_range(1..EPOCH_LENGTH);
        let current_block_3 = current_block_1 + EPOCH_LENGTH;
        let current_block_4 = current_block_3 + rng.gen_range(1..EPOCH_LENGTH);

        assert_eq!(find_epoch_start(EPOCH_LENGTH, current_block_1), current_block_1);
        assert_eq!(find_epoch_start(EPOCH_LENGTH, current_block_2), current_block_1);
        assert_eq!(find_epoch_start(EPOCH_LENGTH, current_block_3), current_block_3);
        assert_eq!(find_epoch_start(EPOCH_LENGTH, current_block_4), current_block_3);
    }

    #[test]
    fn test_get_recent_block_height_or_zero() {
        let block_height = 100_u32;
        let recent_bitcoin_block_header = Some((
            bitcoin::block::Header {
                version: bitcoin::block::Version::default(),
                prev_blockhash: BlockHash::all_zeros(),
                merkle_root: TxMerkleNode::all_zeros(),
                time: 0,
                bits: CompactTarget::default(),
                nonce: 0,
            },
            block_height,
        ));
        assert_eq!(get_recent_block_height_or_zero(recent_bitcoin_block_header), block_height);

        let recent_bitcoin_block_header = None;
        assert_eq!(get_recent_block_height_or_zero(recent_bitcoin_block_header), 0);
    }

    #[test]
    fn test_get_confirmation_depth() {
        assert_eq!(get_confirmation_depth(true), SIGNET_PEGIN_CONFIRMATION_DEPTH);
        assert_eq!(get_confirmation_depth(false), MAINNET_PEGIN_CONFIRMATION_DEPTH);
    }

    #[test]
    fn test_is_testnet() {
        let chain_id = BOTANIX_TESTNET.chain().id();
        assert!(is_testnet(chain_id));

        let chain_id = 1;
        assert!(!is_testnet(chain_id));
    }

    #[test]
    fn test_get_witness_data_from_psbt() {
        let unsigned_tx = bitcoin::Transaction {
            version: 2,
            lock_time: bitcoin::absolute::LockTime::from_height(0).unwrap(),
            input: vec![],
            output: vec![],
        };
        let mut psbt = PartiallySignedTransaction::from_unsigned_tx(unsigned_tx).unwrap();

        let mut input_1 = Input::default();
        input_1.final_script_witness = Some(Witness::default());
        let input_2 = input_1.clone();

        let inputs = vec![input_1, input_2];
        psbt.inputs = inputs.clone();

        assert!(get_witness_data_from_psbt(psbt).len() == inputs.len());
    }
}
