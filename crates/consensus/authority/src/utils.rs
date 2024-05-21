//! Botanix consensus utility functions
use std::time::Duration;

use bitcoin::{hashes::sha256, psbt::Psbt, witness::Witness, BlockHash};
use futures_util::Future;
use reth_botanix_lib::{
    mint_validation::{try_parse_burn_event, BURN_TOPIC, MINT_CONTRACT_ADDRESS, MINT_TOPIC},
    peg_contract::{PeginMeta, PegoutId, PegoutData},
};
use reth_interfaces::sync::SyncStateProvider;
use reth_network::NetworkHandle;
use reth_primitives::{constants::eip225::EPOCH_LENGTH, Bloom, BloomInput};
use reth_provider::BlockReaderIdExt;
use reth_rpc_types::BlockHashOrNumber;
use tracing::{error, info};
use uuid::Uuid;

use btcserverlib::extended_client::{BtcServerExtendedClient, GrpcClientError};
use client::{MakeTxRequest, NotifyPeginRequest, PegoutRequest,SigningPackage};

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

/// Repersents an error related to frost operations
#[derive(Debug, thiserror::Error)]
pub(crate) enum FrostParseError {
    /// Failed to notify btc server about pegin
    #[error("Invalid frost peer id")]
    InvalidFrostPeerId,
    #[error("Invalid frost signing session id")]
    InvalidSigningSessionId,
}

// send pegouts to the btc server and recieve a psbt
// TODO better name for this function
pub(crate) async fn call_get_psbt(
    btc_server: &mut BtcServerExtendedClient,
    pegouts: &[(PegoutId, PegoutData)],
    signing_session_id: &SigningSessionId,
    bitcoin_checkpoint: BlockHash,
    utxo_merkle_root: sha256::Hash,
    botanix_height: u64,
) -> Result<SigningPackage, GrpcClientError> {
    let req = MakeTxRequest {
        new_pegouts: pegouts
            .iter()
            .map(|(id, pegout)| PegoutRequest {
                pegout_id: id.as_bytes().to_vec(),
                script_pubkey: pegout.destination.script_pubkey().to_bytes(),
                amount: pegout.amount.to_sat(),
                botanix_height: botanix_height,
            })
            .collect(),
        signing_session_id: signing_session_id.to_vec(),
        checkpoint_block_hash: bitcoin_checkpoint[..].to_vec(),
        utxo_merkle_root: utxo_merkle_root[..].to_vec(),
    };

    btc_server.get_psbt(req).await
}

pub(crate) async fn call_notify_pegin(
    btc_server: &mut BtcServerExtendedClient,
    pegin: &PeginMeta,
) -> Result<(), GrpcClientError> {
    let request = NotifyPeginRequest {
        utxo_txid: pegin.outpoint.txid.to_string(),
        utxo_vout: pegin.outpoint.vout,
        eth_address: hex::encode(pegin.address),
        output: bitcoin::consensus::serialize(pegin.txout()),
    };
    let _ = btc_server.notify_pegin(request).await?;
    Ok(())
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
) -> Result<Vec<(PegoutId, PegoutData)>, EpochPegoutsError> {
    let mut ret = Vec::new();
    let start_block = find_epoch_start(EPOCH_LENGTH, best_block) - 1;
    for block in start_block..=best_block {
        let block = match client.block_by_number(block) {
            Ok(Some(block)) if bloom_contains_pegout(block.header.logs_bloom) => block,
            Ok(Some(_)) => continue,
            Ok(None) => {
                error!("Block {} not found", block);
                return Err(EpochPegoutsError::FailedToFetchPegouts);
            }
            Err(e) => {
                error!("Error fetching block {}: {}", block, e);
                return Err(EpochPegoutsError::FailedToFetchPegouts);
            }
        };

        for tx in &block.body {
            let txhash = tx.hash();
            let receipt = match client.receipt_by_hash(txhash) {
                Ok(Some(r)) => r,
                _ => continue,
            };
            if !receipt.success {
                continue;
            }
            let mut idx = 0;
            for log in receipt.logs {
                if let Some(p) = try_parse_burn_event(&log, btc_network).expect("already checked") {
                    let id = PegoutId::new(txhash, idx);
                    idx += 1;
                    ret.push((id, p));
                }
            }
        }
    };
    Ok(ret)
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

#[cfg(test)]
mod test {
    use std::str::FromStr;

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
}
