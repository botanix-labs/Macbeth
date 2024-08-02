//! Botanix consensus utility functions
use bitcoin::{hashes::sha256, psbt::Psbt, witness::Witness, BlockHash};
use btcserverlib::extended_client::BtcServerExtendedClient;
use client::{MakeTxRequest, NotifyPeginsRequest, Output, ScriptBuf, SigningPackage, TxOut, Utxo};
use futures_util::Future;
use reth_botanix_lib::{
    mint_validation::{
        parse_pegin_reth_log_topic, parse_pegout_reth_log_topic, GenesisContractEvents, BURN_TOPIC,
        MINT_CONTRACT_ADDRESS, MINT_TOPIC,
    },
    peg_contract::{PeginMeta, PegoutData},
};
use reth_interfaces::sync::SyncStateProvider;
use reth_network::NetworkHandle;
use reth_primitives::{constants::eip225::EPOCH_LENGTH, hex, Bloom, BloomInput, Log, Receipt};
use reth_provider::{BlockReaderIdExt, BundleStateWithReceipts};
use reth_rpc_types::BlockHashOrNumber;
use std::{
    fs::read_to_string,
    path::{Path, PathBuf},
    time::Duration,
};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Checks if the network is undergoing an active sync or not
pub fn is_active_sync_in_progress(network_handle: &NetworkHandle) -> bool {
    network_handle.is_syncing() || network_handle.is_initially_syncing()
}

/// Function for retrying an async closure with retries and delays
pub async fn retry_exec<T, E, F, Fut>(
    fut: F,
    max_retries: u32,
    retry_delay: Duration,
) -> Result<T, E>
where
    E: std::error::Error,
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut retries = 0;
    loop {
        match fut().await {
            Ok(result) => return Ok(result),
            Err(e) if retries < max_retries => {
                error!("Error retrying the execution {:?}", e);
                retries += 1;
                tokio::time::sleep(retry_delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Function for retrying an async closure with retries and delays
pub async fn retry_future<F, Fut, T, E>(
    mut future_factory: F,
    max_retries: usize,
    retry_delay: Duration,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut attempts = 0;
    loop {
        let fut = future_factory();
        match fut.await {
            Ok(value) => return Ok(value),
            Err(_) if attempts < max_retries => {
                attempts += 1;
                tokio::time::sleep(retry_delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// 32 byte signing session id used by the frost coordinator to identify a signing session
/// not consensus critical
pub type SigningSessionId = [u8; 32];

/// Repersents an error while processing a botanix log
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessBotanixLogError {
    /// Failed to notify btc server about pegin
    #[error("Failed to notify btc server about pegin")]
    NotifyPeginFailure(tonic::Status),
    #[error("Failed to make pegout tx: {0}")]
    MakePegoutTxFailure(tonic::Status),
    #[error("Failed to parse pegout data")]
    FailedToParsePegout,
}

/// Repersents an error related to frost operations
#[derive(Debug, thiserror::Error)]
pub(crate) enum FrostParseError {
    /// Failed to notify btc server about pegin
    #[error("Invalid frost peer id")]
    InvalidFrostPeerId,
    #[error("Invalid frost signing session id")]
    InvalidSigningSessionId,
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
pub(crate) fn make_tx_request_for_pegout_in_receipt(
    receipt: Receipt,
    btc_network: bitcoin::Network,
) -> Option<PegoutData> {
    if !receipt.success {
        info!(target: "consensus::authority", "Receipt status code is not success {:?}", receipt);
        return None;
    }

    for log in receipt.logs {
        if let Some(pegout_data) = get_pegout_data(log, btc_network) {
            return Some(pegout_data);
        }
    }

    None
}

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
/// * `pegin_conf_depth` - the number of confirmations required for a pegin
///
/// # Returns
///
/// Returns `Ok(Vec<PegoutData>)` if the processing is successful, otherwise returns an error of
/// type `ProcessBotanixLogError`.
pub(crate) async fn process_receipts(
    btc_server: &mut BtcServerExtendedClient,
    bundle_state: &BundleStateWithReceipts,
    recent_bitcoin_block_height: u32,
    btc_network: bitcoin::Network,
    pegin_conf_depth: u32,
) -> Result<Vec<PegoutData>, ProcessBotanixLogError> {
    let mut pegouts: Vec<PegoutData> = Vec::new();
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
                        &receipt.logs,
                        btc_network,
                        pegin_conf_depth,
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

/// Search a log for a pegout and return [PegoutData] for a burn request
///
/// # Arguments
///
/// * `log` - The log to search for a pegout.
///
/// # Returns
///
/// Returns `Some(PegoutData)` if a pegout is found in the log, otherwise returns `None`.
fn get_pegout_data(log: Log, btc_network: bitcoin::Network) -> Option<PegoutData> {
    for topic in &log.topics().to_vec() {
        match GenesisContractEvents::try_from(*topic) {
            Ok(GenesisContractEvents::MintingEvent) => continue,
            Ok(GenesisContractEvents::BurnEvent) => {
                return Some(
                    parse_pegout_reth_log_topic(&log, btc_network).expect("valid pegout request"),
                );
            }
            Err(e) => {
                debug!(target: "consensus::authority", ?e, "Non burn event");
                continue;
            }
        }
    }
    None
}

// send pegouts to the btc server and recieve a psbt
// TODO better name for this function
pub(crate) async fn get_psbt(
    btc_server: &mut BtcServerExtendedClient,
    pegouts: &[PegoutData],
    signing_session_id: &SigningSessionId,
    bitcoin_checkpoint: BlockHash,
    utxo_merkle_root: sha256::Hash,
) -> Result<SigningPackage, ProcessBotanixLogError> {
    let req = MakeTxRequest {
        outputs: pegouts
            .iter()
            .map(|pegout| Output {
                address: pegout.destination.to_string(),
                value: pegout.amount.to_sat(),
            })
            .collect(),
        signing_session_id: signing_session_id.to_vec(),
        checkpoint_block_hash: bitcoin_checkpoint[..].to_vec(),
        utxo_merkle_root: utxo_merkle_root[..].to_vec(),
    };

    match btc_server.get_psbt(req).await {
        Ok(response) => {
            // start the frost signing session
            Ok(response)
        }
        Err(e) => {
            error!(target: "consensus::authority", ?e, "Failed to make pegout tx");
            Err(ProcessBotanixLogError::MakePegoutTxFailure(e.to_tonic_status()))
        }
    }
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
/// * `bitcoin_checkpoint_height` - the bitcoin block height deeply confirmed for pegins.
/// * `pegin_conf_depth` - the number of confirmations required for a pegin
///
/// # Returns
///
/// Returns `Ok(Option<PegoutData>)` if the processing is successful, otherwise returns an error
/// of type `ProcessBotanixLogError`.
async fn process_botanix_log(
    btc_server: &mut BtcServerExtendedClient,
    log: &Log,
    bitcoin_checkpoint_height: u32,
    receipt_logs: &[Log],
    btc_network: bitcoin::Network,
    _pegin_conf_depth: u32,
) -> Result<Option<PegoutData>, ProcessBotanixLogError> {
    let mut pegout: Option<PegoutData> = None;
    for topic in &log.topics().to_vec() {
        match GenesisContractEvents::try_from(*topic) {
            Ok(GenesisContractEvents::MintingEvent) => {
                info!(target: "consensus::authority", "Parsing and sending minting event to btc_server");
                let pegin_data = parse_pegin_reth_log_topic(log, receipt_logs)
                    .expect("passed evm check should pass this parse attempt");
                // enforce required confirmation depth by network
                if pegin_data.bitcoin_block_height >= bitcoin_checkpoint_height {
                    warn!(target: "consensus::authority", "pegin confirmation depth not met, skipping");
                    continue;
                }

                let utxos = pegin_data.meta.iter().map(utxo_from_pegin_meta).collect();

                let request = NotifyPeginsRequest { utxos };
                btc_server
                    .notify_pegins(request)
                    .await
                    .map_err(|e| ProcessBotanixLogError::NotifyPeginFailure(e.to_tonic_status()))?;
                info!(target: "consensus::authority", "notifying btc server about pegin utxos");
            }
            Ok(GenesisContractEvents::BurnEvent) => {
                // validate pegout
                info!(target: "consensus::authority", "Validating pegout");
                match parse_pegout_reth_log_topic(log, btc_network) {
                    Ok(parsed_pegout) => {
                        pegout = Some(parsed_pegout);
                    }
                    Err(e) => {
                        error!(target: "consensus::authority", ?e, "Failed to parse pegout");
                        return Err(ProcessBotanixLogError::FailedToParsePegout);
                    }
                }
            }
            Err(e) => {
                debug!(target: "consensus::authority", ?e, "Non-genesis contract event");
                continue;
            }
        }
    }
    Ok(pegout)
}

fn utxo_from_pegin_meta(pegin_meta: &PeginMeta) -> Utxo {
    let tx_out = pegin_meta.tx.output.get(pegin_meta.outpoint.vout as usize).expect("valid vout");
    let serialized_script_pub_key = bitcoin::consensus::serialize(&tx_out.script_pubkey);
    Utxo {
        outpoint: Some(client::OutPoint {
            txid: bitcoin::consensus::serialize(&pegin_meta.outpoint.txid),
            vout: pegin_meta.outpoint.vout,
        }),
        output: Some(TxOut {
            script_pubkey: Some(ScriptBuf { script: serialized_script_pub_key }),
            value: tx_out.value.to_sat(),
        }),
        eth_address: hex::encode(pegin_meta.address),
    }
}

fn bloom_contains_minting_contract_address(bloom: Bloom) -> bool {
    bloom.contains_input(BloomInput::Raw(MINT_CONTRACT_ADDRESS.as_ref()))
}

pub(crate) fn bloom_contains_pegout(bloom: Bloom) -> bool {
    bloom_contains_minting_contract_address(bloom) &&
        bloom.contains_input(BloomInput::Raw(BURN_TOPIC.as_ref()))
}

#[allow(dead_code)]
pub(crate) fn bloom_contains_pegin(bloom: Bloom) -> bool {
    bloom_contains_minting_contract_address(bloom) &&
        bloom.contains_input(BloomInput::Raw(MINT_TOPIC.as_ref()))
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

pub(crate) fn get_witness_data_from_psbt(psbt: Psbt) -> Vec<Witness> {
    psbt.inputs.iter().filter_map(|input| input.final_script_witness.clone()).collect()
}

// Deserializes a Frost peer ID.
///
/// # Arguments
///
/// * `id` - The peer ID to be decoded.
///
/// # Returns
///
/// Returns a `Result` containing the serialized Frost identifier if successful, or an `Error` if
/// the peer ID is invalid.
/// use frost_secp256k1_tr
pub(crate) fn deserialize_frost_peer_id(
    id: Vec<u8>,
) -> Result<frost_secp256k1_tr::Identifier, FrostParseError> {
    if id.len() != 32 {
        return Err(FrostParseError::InvalidFrostPeerId);
    }
    let peer_id_bytes: &[u8; 32] =
        id.as_slice().try_into().map_err(|_e| FrostParseError::InvalidFrostPeerId)?;

    let frost_id = frost_secp256k1_tr::Identifier::deserialize(peer_id_bytes)
        .map_err(|_e| FrostParseError::InvalidFrostPeerId)?;

    Ok(frost_id)
}

pub(crate) fn parse_signing_session_id(session_id: &[u8]) -> Result<[u8; 32], FrostParseError> {
    if session_id.len() != 32 {
        return Err(FrostParseError::InvalidSigningSessionId);
    }
    let mut session_id_array = [0u8; 32];
    session_id_array.copy_from_slice(session_id);
    Ok(session_id_array)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum EpochPegoutsError {
    #[error("Failed to fetch pegouts for an epoch")]
    FailedToFetchPegouts,
}

/// Returns all pegouts in an epoch iterating through an inclusive block range
///
/// # Arguments
///
/// * `current_block` - The current block number
/// * `client` - Reth database client
///
/// # Returns
///
/// A vector of [PegoutData] representing the pegouts in the epoch
pub(crate) async fn epoch_pegouts(
    best_block: u64,
    client: &impl BlockReaderIdExt,
    btc_network: bitcoin::Network,
) -> Result<Vec<PegoutData>, EpochPegoutsError> {
    let start_block = find_epoch_start(EPOCH_LENGTH, best_block);
    let mut pegouts: Vec<PegoutData> = vec![];
    for block in start_block..=best_block {
        match client.block_by_number(block) {
            Ok(Some(block)) if bloom_contains_pegout(block.header.logs_bloom) => {
                match client.receipts_by_block(BlockHashOrNumber::Number(block.header.number)) {
                    Ok(Some(receipts)) => {
                        for receipt in receipts {
                            if let Some(p) =
                                make_tx_request_for_pegout_in_receipt(receipt, btc_network)
                            {
                                pegouts.push(p);
                            }
                        }
                    }
                    Ok(None) => {
                        info!("No receipts found for block {:?}", block);
                        continue;
                    }
                    Err(e) => {
                        error!("Error fetching receipts for block {:?}: {}", block, e);
                        return Err(EpochPegoutsError::FailedToFetchPegouts);
                    }
                }
            }
            Ok(Some(_)) => {
                info!("No pegouts found in block {}", block);
                continue;
            }
            Ok(None) => {
                error!("Block {} not found", block);
                return Err(EpochPegoutsError::FailedToFetchPegouts);
            }
            Err(e) => {
                error!("Error fetching block {}: {}", block, e);
                return Err(EpochPegoutsError::FailedToFetchPegouts);
            }
        }
    }

    Ok(pegouts)
}

/// Errors that can occur while generating a signing session ID
#[derive(Debug, thiserror::Error)]
pub(crate) enum GenerateSigningSesssionIdError {
    #[error("Failed to generate hash")]
    HashError(#[from] std::io::Error),
}

// Generates a signing session id using a uuid v4 generator
pub(crate) fn generate_signing_session_id(
) -> Result<SigningSessionId, GenerateSigningSesssionIdError> {
    let id = Uuid::new_v4();
    let hex_string = id.simple().to_string(); // Removing dashes, results in 32 hex digits
    let bytes: Vec<u8> = hex_string.bytes().collect();
    let bytes_array: [u8; 32] = bytes.try_into().expect("Expected a Vec<u8> of length 32");
    Ok(bytes_array)
}

/// Checks Minting.sol deployed bytecode against known and verified bytecode
pub fn is_known_minting_contract(
    precompiled_bytecode: String,
    deployed_bytecode: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    if precompiled_bytecode != hex::encode(deployed_bytecode) {
        error!("Precompiled Minting contract bytecode: {}", precompiled_bytecode);
        error!("Deployed Minting contract bytecode: {}", hex::encode(deployed_bytecode));
        return Err("Minting contract bytecode does not match known bytecode".into());
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use std::{env, str::FromStr};

    use bitcoin::{
        psbt::{Input, Psbt},
        transaction::Version,
    };
    use rand::Rng;
    use reth_primitives::{address, b256, bytes, Header, B256, U256};

    use super::*;

    #[test]
    fn test_uuid() {
        assert!(generate_signing_session_id().unwrap().len() == 32);
    }

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
    fn test_get_witness_data_from_psbt() {
        let unsigned_tx = bitcoin::Transaction {
            version: Version(2),
            lock_time: bitcoin::absolute::LockTime::from_height(0).unwrap(),
            input: vec![],
            output: vec![],
        };
        let mut psbt = Psbt::from_unsigned_tx(unsigned_tx).unwrap();

        let mut input_1 = Input::default();
        input_1.final_script_witness = Some(Witness::default());
        let input_2 = input_1.clone();

        let inputs = vec![input_1, input_2];
        psbt.inputs = inputs.clone();

        assert!(get_witness_data_from_psbt(psbt).len() == inputs.len());
    }

    #[test]
    fn test_is_known_mint_contract() {
        env::set_current_dir("../../../contracts").unwrap();

        // test happy path
        let precompiled_bytecode = String::from("60806040526004361061003f5760003560e01c80635fe03f45146100445780636f194dc914610066578063a5d0bb93146100b3578063a8de6d8c146100d6575b600080fd5b34801561005057600080fd5b5061006461005f36600461048c565b6100fd565b005b34801561007257600080fd5b50610099610081366004610515565b60006020819052908152604090205463ffffffff1681565b60405163ffffffff90911681526020015b60405180910390f35b6100c66100c1366004610537565b61034c565b60405190151581526020016100aa565b3480156100e257600080fd5b506100ef6402540be40081565b6040519081526020016100aa565b60005a6001600160a01b03881660009081526020819052604090205490915063ffffffff9081169086161161018b5760405162461bcd60e51b815260206004820152602960248201527f7573657220626974636f696e426c6f636b486569676874206e6565647320746f60448201526820696e63726561736560b81b60648201526084015b60405180910390fd5b6001600160a01b03871660008181526020819052604090819020805463ffffffff191663ffffffff8916179055517f922344dc04648c0ce028ecdf9b2c9eed9a6794dbb47b777b54b0cfe069f128aa906101ec9089908990899089906105cc565b60405180910390a260003a61046d60036107d3615208805a61020e9089610612565b610218919061062b565b610222919061062b565b61022c919061062b565b610236919061062b565b610240919061062b565b61024a919061063e565b90508681111561029c5760405162461bcd60e51b815260206004820152601c60248201527f547820636f7374206578636565647320706567696e20616d6f756e74000000006044820152606401610182565b6102a68188610612565b6040519097506001600160a01b0389169088156108fc029089906000818181858888f193505050501580156102df573d6000803e3d6000fd5b506040516001600160a01b0384169082156108fc029083906000818181858888f19350505050158015610316573d6000803e3d6000fd5b5060405187907f8e37eb2ee3a6f3c8b13b8973588daad75a4ce752de14c00006bd8247f4e212e890600090a25050505050505050565b600061035f6402540be40061014a61063e565b34116103d35760405162461bcd60e51b815260206004820152603860248201527f56616c7565206d7573742062652067726561746572207468616e20647573742060448201527f616d6f756e74206f662033333020736174732f764279746500000000000000006064820152608401610182565b336001600160a01b03167f17f87987da8ca71c697791dcfd190d07630cf17bf09c65c5a59b8277d9fe17153487878787604051610414959493929190610655565b60405180910390a2506001949350505050565b80356001600160a01b038116811461043e57600080fd5b919050565b60008083601f84011261045557600080fd5b50813567ffffffffffffffff81111561046d57600080fd5b60208301915083602082850101111561048557600080fd5b9250929050565b60008060008060008060a087890312156104a557600080fd5b6104ae87610427565b955060208701359450604087013563ffffffff811681146104ce57600080fd5b9350606087013567ffffffffffffffff8111156104ea57600080fd5b6104f689828a01610443565b9094509250610509905060808801610427565b90509295509295509295565b60006020828403121561052757600080fd5b61053082610427565b9392505050565b6000806000806040858703121561054d57600080fd5b843567ffffffffffffffff8082111561056557600080fd5b61057188838901610443565b9096509450602087013591508082111561058a57600080fd5b5061059787828801610443565b95989497509550505050565b81835281816020850137506000828201602090810191909152601f909101601f19169091010190565b84815263ffffffff841660208201526060604082015260006105f26060830184866105a3565b9695505050505050565b634e487b7160e01b600052601160045260246000fd5b81810381811115610625576106256105fc565b92915050565b80820180821115610625576106256105fc565b8082028115828204841417610625576106256105fc565b85815260606020820152600061066f6060830186886105a3565b82810360408401526106828185876105a3565b9897505050505050505056fea2646970667358221220e595d50ab7b94f4eebd02147af9ae6beb1b9a3e3dba6fc707c45d64e8129736764736f6c63430008150033");
        let deployed_bytecode = [
            96, 128, 96, 64, 82, 96, 4, 54, 16, 97, 0, 63, 87, 96, 0, 53, 96, 224, 28, 128, 99, 95,
            224, 63, 69, 20, 97, 0, 68, 87, 128, 99, 111, 25, 77, 201, 20, 97, 0, 102, 87, 128, 99,
            165, 208, 187, 147, 20, 97, 0, 179, 87, 128, 99, 168, 222, 109, 140, 20, 97, 0, 214,
            87, 91, 96, 0, 128, 253, 91, 52, 128, 21, 97, 0, 80, 87, 96, 0, 128, 253, 91, 80, 97,
            0, 100, 97, 0, 95, 54, 96, 4, 97, 4, 140, 86, 91, 97, 0, 253, 86, 91, 0, 91, 52, 128,
            21, 97, 0, 114, 87, 96, 0, 128, 253, 91, 80, 97, 0, 153, 97, 0, 129, 54, 96, 4, 97, 5,
            21, 86, 91, 96, 0, 96, 32, 129, 144, 82, 144, 129, 82, 96, 64, 144, 32, 84, 99, 255,
            255, 255, 255, 22, 129, 86, 91, 96, 64, 81, 99, 255, 255, 255, 255, 144, 145, 22, 129,
            82, 96, 32, 1, 91, 96, 64, 81, 128, 145, 3, 144, 243, 91, 97, 0, 198, 97, 0, 193, 54,
            96, 4, 97, 5, 55, 86, 91, 97, 3, 76, 86, 91, 96, 64, 81, 144, 21, 21, 129, 82, 96, 32,
            1, 97, 0, 170, 86, 91, 52, 128, 21, 97, 0, 226, 87, 96, 0, 128, 253, 91, 80, 97, 0,
            239, 100, 2, 84, 11, 228, 0, 129, 86, 91, 96, 64, 81, 144, 129, 82, 96, 32, 1, 97, 0,
            170, 86, 91, 96, 0, 90, 96, 1, 96, 1, 96, 160, 27, 3, 136, 22, 96, 0, 144, 129, 82, 96,
            32, 129, 144, 82, 96, 64, 144, 32, 84, 144, 145, 80, 99, 255, 255, 255, 255, 144, 129,
            22, 144, 134, 22, 17, 97, 1, 139, 87, 96, 64, 81, 98, 70, 27, 205, 96, 229, 27, 129,
            82, 96, 32, 96, 4, 130, 1, 82, 96, 41, 96, 36, 130, 1, 82, 127, 117, 115, 101, 114, 32,
            98, 105, 116, 99, 111, 105, 110, 66, 108, 111, 99, 107, 72, 101, 105, 103, 104, 116,
            32, 110, 101, 101, 100, 115, 32, 116, 111, 96, 68, 130, 1, 82, 104, 32, 105, 110, 99,
            114, 101, 97, 115, 101, 96, 184, 27, 96, 100, 130, 1, 82, 96, 132, 1, 91, 96, 64, 81,
            128, 145, 3, 144, 253, 91, 96, 1, 96, 1, 96, 160, 27, 3, 135, 22, 96, 0, 129, 129, 82,
            96, 32, 129, 144, 82, 96, 64, 144, 129, 144, 32, 128, 84, 99, 255, 255, 255, 255, 25,
            22, 99, 255, 255, 255, 255, 137, 22, 23, 144, 85, 81, 127, 146, 35, 68, 220, 4, 100,
            140, 12, 224, 40, 236, 223, 155, 44, 158, 237, 154, 103, 148, 219, 180, 123, 119, 123,
            84, 176, 207, 224, 105, 241, 40, 170, 144, 97, 1, 236, 144, 137, 144, 137, 144, 137,
            144, 137, 144, 97, 5, 204, 86, 91, 96, 64, 81, 128, 145, 3, 144, 162, 96, 0, 58, 97, 4,
            109, 96, 3, 97, 7, 211, 97, 82, 8, 128, 90, 97, 2, 14, 144, 137, 97, 6, 18, 86, 91, 97,
            2, 24, 145, 144, 97, 6, 43, 86, 91, 97, 2, 34, 145, 144, 97, 6, 43, 86, 91, 97, 2, 44,
            145, 144, 97, 6, 43, 86, 91, 97, 2, 54, 145, 144, 97, 6, 43, 86, 91, 97, 2, 64, 145,
            144, 97, 6, 43, 86, 91, 97, 2, 74, 145, 144, 97, 6, 62, 86, 91, 144, 80, 134, 129, 17,
            21, 97, 2, 156, 87, 96, 64, 81, 98, 70, 27, 205, 96, 229, 27, 129, 82, 96, 32, 96, 4,
            130, 1, 82, 96, 28, 96, 36, 130, 1, 82, 127, 84, 120, 32, 99, 111, 115, 116, 32, 101,
            120, 99, 101, 101, 100, 115, 32, 112, 101, 103, 105, 110, 32, 97, 109, 111, 117, 110,
            116, 0, 0, 0, 0, 96, 68, 130, 1, 82, 96, 100, 1, 97, 1, 130, 86, 91, 97, 2, 166, 129,
            136, 97, 6, 18, 86, 91, 96, 64, 81, 144, 151, 80, 96, 1, 96, 1, 96, 160, 27, 3, 137,
            22, 144, 136, 21, 97, 8, 252, 2, 144, 137, 144, 96, 0, 129, 129, 129, 133, 136, 136,
            241, 147, 80, 80, 80, 80, 21, 128, 21, 97, 2, 223, 87, 61, 96, 0, 128, 62, 61, 96, 0,
            253, 91, 80, 96, 64, 81, 96, 1, 96, 1, 96, 160, 27, 3, 132, 22, 144, 130, 21, 97, 8,
            252, 2, 144, 131, 144, 96, 0, 129, 129, 129, 133, 136, 136, 241, 147, 80, 80, 80, 80,
            21, 128, 21, 97, 3, 22, 87, 61, 96, 0, 128, 62, 61, 96, 0, 253, 91, 80, 96, 64, 81,
            135, 144, 127, 142, 55, 235, 46, 227, 166, 243, 200, 177, 59, 137, 115, 88, 141, 170,
            215, 90, 76, 231, 82, 222, 20, 192, 0, 6, 189, 130, 71, 244, 226, 18, 232, 144, 96, 0,
            144, 162, 80, 80, 80, 80, 80, 80, 80, 80, 86, 91, 96, 0, 97, 3, 95, 100, 2, 84, 11,
            228, 0, 97, 1, 74, 97, 6, 62, 86, 91, 52, 17, 97, 3, 211, 87, 96, 64, 81, 98, 70, 27,
            205, 96, 229, 27, 129, 82, 96, 32, 96, 4, 130, 1, 82, 96, 56, 96, 36, 130, 1, 82, 127,
            86, 97, 108, 117, 101, 32, 109, 117, 115, 116, 32, 98, 101, 32, 103, 114, 101, 97, 116,
            101, 114, 32, 116, 104, 97, 110, 32, 100, 117, 115, 116, 32, 96, 68, 130, 1, 82, 127,
            97, 109, 111, 117, 110, 116, 32, 111, 102, 32, 51, 51, 48, 32, 115, 97, 116, 115, 47,
            118, 66, 121, 116, 101, 0, 0, 0, 0, 0, 0, 0, 0, 96, 100, 130, 1, 82, 96, 132, 1, 97, 1,
            130, 86, 91, 51, 96, 1, 96, 1, 96, 160, 27, 3, 22, 127, 23, 248, 121, 135, 218, 140,
            167, 28, 105, 119, 145, 220, 253, 25, 13, 7, 99, 12, 241, 123, 240, 156, 101, 197, 165,
            155, 130, 119, 217, 254, 23, 21, 52, 135, 135, 135, 135, 96, 64, 81, 97, 4, 20, 149,
            148, 147, 146, 145, 144, 97, 6, 85, 86, 91, 96, 64, 81, 128, 145, 3, 144, 162, 80, 96,
            1, 148, 147, 80, 80, 80, 80, 86, 91, 128, 53, 96, 1, 96, 1, 96, 160, 27, 3, 129, 22,
            129, 20, 97, 4, 62, 87, 96, 0, 128, 253, 91, 145, 144, 80, 86, 91, 96, 0, 128, 131, 96,
            31, 132, 1, 18, 97, 4, 85, 87, 96, 0, 128, 253, 91, 80, 129, 53, 103, 255, 255, 255,
            255, 255, 255, 255, 255, 129, 17, 21, 97, 4, 109, 87, 96, 0, 128, 253, 91, 96, 32, 131,
            1, 145, 80, 131, 96, 32, 130, 133, 1, 1, 17, 21, 97, 4, 133, 87, 96, 0, 128, 253, 91,
            146, 80, 146, 144, 80, 86, 91, 96, 0, 128, 96, 0, 128, 96, 0, 128, 96, 160, 135, 137,
            3, 18, 21, 97, 4, 165, 87, 96, 0, 128, 253, 91, 97, 4, 174, 135, 97, 4, 39, 86, 91,
            149, 80, 96, 32, 135, 1, 53, 148, 80, 96, 64, 135, 1, 53, 99, 255, 255, 255, 255, 129,
            22, 129, 20, 97, 4, 206, 87, 96, 0, 128, 253, 91, 147, 80, 96, 96, 135, 1, 53, 103,
            255, 255, 255, 255, 255, 255, 255, 255, 129, 17, 21, 97, 4, 234, 87, 96, 0, 128, 253,
            91, 97, 4, 246, 137, 130, 138, 1, 97, 4, 67, 86, 91, 144, 148, 80, 146, 80, 97, 5, 9,
            144, 80, 96, 128, 136, 1, 97, 4, 39, 86, 91, 144, 80, 146, 149, 80, 146, 149, 80, 146,
            149, 86, 91, 96, 0, 96, 32, 130, 132, 3, 18, 21, 97, 5, 39, 87, 96, 0, 128, 253, 91,
            97, 5, 48, 130, 97, 4, 39, 86, 91, 147, 146, 80, 80, 80, 86, 91, 96, 0, 128, 96, 0,
            128, 96, 64, 133, 135, 3, 18, 21, 97, 5, 77, 87, 96, 0, 128, 253, 91, 132, 53, 103,
            255, 255, 255, 255, 255, 255, 255, 255, 128, 130, 17, 21, 97, 5, 101, 87, 96, 0, 128,
            253, 91, 97, 5, 113, 136, 131, 137, 1, 97, 4, 67, 86, 91, 144, 150, 80, 148, 80, 96,
            32, 135, 1, 53, 145, 80, 128, 130, 17, 21, 97, 5, 138, 87, 96, 0, 128, 253, 91, 80, 97,
            5, 151, 135, 130, 136, 1, 97, 4, 67, 86, 91, 149, 152, 148, 151, 80, 149, 80, 80, 80,
            80, 86, 91, 129, 131, 82, 129, 129, 96, 32, 133, 1, 55, 80, 96, 0, 130, 130, 1, 96, 32,
            144, 129, 1, 145, 144, 145, 82, 96, 31, 144, 145, 1, 96, 31, 25, 22, 144, 145, 1, 1,
            144, 86, 91, 132, 129, 82, 99, 255, 255, 255, 255, 132, 22, 96, 32, 130, 1, 82, 96, 96,
            96, 64, 130, 1, 82, 96, 0, 97, 5, 242, 96, 96, 131, 1, 132, 134, 97, 5, 163, 86, 91,
            150, 149, 80, 80, 80, 80, 80, 80, 86, 91, 99, 78, 72, 123, 113, 96, 224, 27, 96, 0, 82,
            96, 17, 96, 4, 82, 96, 36, 96, 0, 253, 91, 129, 129, 3, 129, 129, 17, 21, 97, 6, 37,
            87, 97, 6, 37, 97, 5, 252, 86, 91, 146, 145, 80, 80, 86, 91, 128, 130, 1, 128, 130, 17,
            21, 97, 6, 37, 87, 97, 6, 37, 97, 5, 252, 86, 91, 128, 130, 2, 129, 21, 130, 130, 4,
            132, 20, 23, 97, 6, 37, 87, 97, 6, 37, 97, 5, 252, 86, 91, 133, 129, 82, 96, 96, 96,
            32, 130, 1, 82, 96, 0, 97, 6, 111, 96, 96, 131, 1, 134, 136, 97, 5, 163, 86, 91, 130,
            129, 3, 96, 64, 132, 1, 82, 97, 6, 130, 129, 133, 135, 97, 5, 163, 86, 91, 152, 151,
            80, 80, 80, 80, 80, 80, 80, 80, 86, 254, 162, 100, 105, 112, 102, 115, 88, 34, 18, 32,
            229, 149, 213, 10, 183, 185, 79, 78, 235, 208, 33, 71, 175, 154, 230, 190, 177, 185,
            163, 227, 219, 166, 252, 112, 124, 69, 214, 78, 129, 41, 115, 103, 100, 115, 111, 108,
            99, 67, 0, 8, 21, 0, 51,
        ];

        assert!(is_known_minting_contract(precompiled_bytecode.clone(), &deployed_bytecode).is_ok());

        // test fail path
        let deployed_bytecode = "not known minting contract bytecode".as_bytes();
        assert!(is_known_minting_contract(precompiled_bytecode, deployed_bytecode).is_err());
    }
}
