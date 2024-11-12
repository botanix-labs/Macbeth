//! Botanix consensus utility functions
use bitcoin::{
    consensus::Encodable,
    hashes::{sha256, Hash},
    psbt::Psbt,
    witness::Witness,
    Address, Amount, BlockHash,
};
use btcserverlib::{
    extended_client::{BtcServerExtendedClient, GrpcClientError},
    pegout_id::PegoutId,
};
use client::{
    MakeTxRequest, NotifyPeginsRequest, NotifyPegoutRequest, ScriptBuf, SigningPackage, TxOut, Utxo,
};
use futures_util::Future;
use reth_btc_wallet::psbt::PsbtOutputExt;
use reth_network::{NetworkHandle, NetworkInfo};
use reth_primitives::{
    botanix::{
        mint_validation::{try_parse_burn_event, BURN_TOPIC, MINT_CONTRACT_ADDRESS, MINT_TOPIC},
        peg_contract::{PeginMeta, PegoutData, PegoutWithId},
    },
    constants::EPOCH_LENGTH,
    Bloom, BloomInput,
};
use reth_provider::{BlockReaderIdExt, ReceiptProvider};
use reth_revm::primitives::FixedBytes;
use reth_rpc_types::BlockHashOrNumber;
use std::{fmt::Debug, time::Duration};
use tracing::{debug, error, info};
use uuid::Uuid;

/// Checks if the network is undergoing an active sync or not
pub fn is_active_sync_in_progress(network_handle: &NetworkHandle) -> bool {
    network_handle.is_syncing() || network_handle.is_initially_syncing()
}

/// Function for retrying an async closure with retries and delays
pub async fn retry_exec<T, E, F, Fut>(
    method_name: &str,
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
                error!(
                    "Error retrying the execution of function {:?}. Error: {:?}",
                    method_name, e
                );
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

/// receive a psbt containing all pending pegouts awaiting signing
pub(crate) async fn get_psbt(
    btc_server: &mut BtcServerExtendedClient,
    signing_session_id: &SigningSessionId,
    bitcoin_checkpoint: BlockHash,
) -> Result<SigningPackage, GrpcClientError> {
    let req = MakeTxRequest {
        signing_session_id: signing_session_id.to_vec(),
        checkpoint_block_hash: bitcoin_checkpoint[..].to_vec(),
    };

    btc_server.get_psbt(req).await
}

pub(crate) async fn call_notify_pegin(
    btc_server: &mut BtcServerExtendedClient,
    pegins: &[PeginMeta],
) -> Result<(), GrpcClientError> {
    if pegins.is_empty() {
        return Ok(());
    }
    let utxos = pegins.iter().map(utxo_from_pegin_meta).collect();
    let request = NotifyPeginsRequest { utxos };
    btc_server.notify_pegins(request).await?;
    info!(target: "consensus::authority", "notifying btc server about pegin utxos");
    Ok(())
}

pub(crate) async fn call_notify_pegout(
    btc_server: &mut BtcServerExtendedClient,
    pegouts: &[PegoutWithId],
    height: u64,
) -> Result<(), GrpcClientError> {
    if pegouts.is_empty() {
        return Ok(());
    }

    // TODO: do we want to modify NotifyPegoutRequest to take a list of pegouts?
    // loop through pegouts and notify
    for pegout in pegouts {
        let request = NotifyPegoutRequest {
            pegout_id: pegout.id.as_bytes().to_vec(),
            spk: pegout.data.destination.script_pubkey().to_bytes().to_vec(),
            amount: pegout.data.amount.to_sat(),
            height,
        };
        btc_server.notify_pegout(request).await?;
    }

    Ok(())
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
#[allow(dead_code)]
pub(crate) fn find_epoch_start(epoch_length: u64, current_block_number: u64) -> u64 {
    let mut start_block_number = current_block_number;
    while start_block_number % epoch_length != 0 {
        start_block_number -= 1;
    }
    start_block_number
}

#[allow(dead_code)]
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

pub(crate) fn parse_signing_session_id(
    session_id: &FixedBytes<32>,
) -> Result<[u8; 32], FrostParseError> {
    if session_id.len() != 32 {
        return Err(FrostParseError::InvalidSigningSessionId);
    }
    let mut session_id_array = [0u8; 32];
    session_id_array.copy_from_slice(session_id.as_slice());
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
#[allow(dead_code)]
pub(crate) async fn epoch_pegouts(
    best_block: u64,
    client: &impl BlockReaderIdExt,
    btc_network: bitcoin::Network,
) -> Result<Vec<PegoutData>, EpochPegoutsError> {
    let start_block = find_epoch_start(EPOCH_LENGTH, best_block);
    let mut pegouts = Vec::new();
    for block in start_block..=best_block {
        match client.block_by_number(block) {
            Ok(Some(block)) if bloom_contains_pegout(block.header.logs_bloom) => {
                match client.receipts_by_block(BlockHashOrNumber::Number(block.header.number)) {
                    Ok(Some(receipts)) => {
                        for receipt in receipts {
                            if !receipt.success {
                                continue;
                            }
                            for log in receipt.logs {
                                if let Ok(Some(p)) = try_parse_burn_event(&log, btc_network) {
                                    pegouts.push(p);
                                }
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
            Ok(Some(_)) => continue,
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

/// Generates a signing session id using a uuid v4 generator
#[allow(dead_code)]
pub(crate) fn generate_signing_session_id(
) -> Result<SigningSessionId, GenerateSigningSesssionIdError> {
    let id = Uuid::new_v4();
    let hex_string = id.simple().to_string(); // Removing dashes, results in 32 hex digits
    let bytes: Vec<u8> = hex_string.bytes().collect();
    let bytes_array: [u8; 32] = bytes.try_into().expect("Expected a Vec<u8> of length 32");
    Ok(bytes_array)
}

/// Repersents an error related to utxo operations
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub(crate) enum UtxoMerkelRootError {
    #[error("Unparsable tx id")]
    UnparsableTxId,
    #[error("Outpoint encoding")]
    OutpointEncoding,
    #[error("Error calculating merkle root")]
    BadMerkleRoot,
    #[error("Missing UTXO outpoint")]
    MissingOutpoint,
}

/// Generates a utxo merkel root from a list of utxos
#[allow(dead_code)]
pub(crate) fn generate_utxo_merkel_root(
    peer_utxos: &[Utxo],
) -> Result<bitcoin::hashes::sha256::Hash, UtxoMerkelRootError> {
    if peer_utxos.is_empty() {
        return Ok(bitcoin::hashes::sha256::Hash::all_zeros());
    }

    let mut utxos = peer_utxos
        .iter()
        .map(|u| {
            let mut engine = sha256::Hash::engine();
            let ot = u.clone().outpoint.ok_or(UtxoMerkelRootError::MissingOutpoint)?;
            let tx_id = bitcoin::hash_types::Txid::from_slice(&ot.txid)
                .ok()
                .ok_or(UtxoMerkelRootError::UnparsableTxId)?;
            let btc_outpoint = bitcoin::transaction::OutPoint::new(tx_id, ot.vout);
            btc_outpoint
                .consensus_encode(&mut engine)
                .map_err(|_| UtxoMerkelRootError::OutpointEncoding)?;
            Ok(sha256::Hash::from_engine(engine))
        })
        .collect::<Result<Vec<_>, UtxoMerkelRootError>>()?;

    // sort the utxos
    utxos.sort();

    // compute the utxo set hash root
    let root = bitcoin::merkle_tree::calculate_root(utxos.into_iter())
        .ok_or(UtxoMerkelRootError::BadMerkleRoot)?;
    Ok(root)
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

#[derive(Debug, thiserror::Error)]
/// Represents errors that can occur during psbt validation
pub enum PsbtValidationError {
    #[error("Failed to validate psbt by ids: {0}")]
    /// Failed to validate psbt by ids
    FailedToValidatePsbtByIds(String),
}

/// Extract pegouts ids from psbt
pub fn extract_pegout_ids(psbt: &Psbt) -> Vec<PegoutId> {
    psbt.outputs
        .iter()
        .filter_map(|output| match output.pegout_id() {
            Some(pegout_id) => PegoutId::from_bytes(pegout_id.as_slice()).ok(),
            _ => None,
        })
        .collect()
}

/// Validate psbt contains the correct output
pub fn validate_psbt_by_output(
    psbt: &Psbt,
    destination: &Address,
    amount: Amount,
) -> Result<(), PsbtValidationError> {
    debug!(target: "consensus::authority::frost_task::validate_psbt_by_ids", "Validating {} outputs in psbt", psbt.outputs.len());
    match psbt.clone().extract_tx() {
        Ok(transaction) => {
            match transaction.output.iter().find(|output| {
                output.script_pubkey == destination.script_pubkey() &&
                            // TODO: strict value checking will need to account for fees
                            output.value == amount
            }) {
                Some(_) => {
                    debug!(target: "consensus::authority::frost_task::validate_psbt_by_ids", "Found matching output in psbt");
                    Ok(())
                }
                None => {
                    error!(target: "consensus::authority::frost_task::validate_psbt_by_ids", "Failed to find matching output in psbt");
                    Err(PsbtValidationError::FailedToValidatePsbtByIds(String::from(
                        "Failed to find matching output in psbt",
                    )))
                }
            }
        }
        Err(e) => {
            error!(target: "consensus::authority::frost_task::validate_psbt_by_ids", "Failed to extract transaction from psbt {:?}", e);
            Err(PsbtValidationError::FailedToValidatePsbtByIds(String::from(
                "Failed to extract transaction from psbt",
            )))
        }
    }
}

/// Validate psbt by pegout ids
pub async fn validate_psbt_by_ids(
    client: impl ReceiptProvider + Clone,
    btc_network: bitcoin::Network,
    psbt: &Psbt,
) -> Result<(), PsbtValidationError> {
    let pegout_ids = extract_pegout_ids(psbt);

    if pegout_ids.is_empty() {
        error!(target: "consensus::authority::frost_task::validate_psbt_by_ids", "No pegout ids found in psbt");
        return Err(PsbtValidationError::FailedToValidatePsbtByIds(String::from(
            "No pegout ids found in psbt",
        )));
    }

    // get pegouts from db
    for PegoutId { txid, idx } in pegout_ids.iter() {
        let log =
            client
            .receipt_by_hash(txid.into())
            .ok()
            .flatten()
            .and_then(|receipts| receipts.logs.get(*idx as usize).cloned())
            .ok_or_else(|| {
                error!(target: "consensus::authority::frost_task::validate_psbt_by_ids", "Failed to get log from receipts");
                PsbtValidationError::FailedToValidatePsbtByIds(String::from("Failed to get log from receipts"))
            })?;

        let PegoutData { amount, destination, network: _} = try_parse_burn_event(&log, btc_network)
            .map_err(|e| {
                error!(target: "consensus::authority::frost_task::validate_psbt_by_ids", "Failed to parse burn event {:?}", e);
                PsbtValidationError::FailedToValidatePsbtByIds(String::from("Failed to parse burn event"))
            })?
            .ok_or_else(|| {
                error!(target: "consensus::authority::frost_task::validate_psbt_by_ids", "Failed to get pegout data from burn event");
                PsbtValidationError::FailedToValidatePsbtByIds(String::from("Failed to get pegout data from burn event"))
            })?;

        // check if a corresponding output exists in the psbt
        validate_psbt_by_output(psbt, &destination, amount)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use bitcoin::{
        absolute::LockTime,
        psbt::{Input, Psbt},
        transaction::Version,
        FeeRate, OutPoint, Sequence, Transaction, TxIn, Txid,
    };
    use rand::{thread_rng, Rng, RngCore};
    use reth_primitives::{
        address, b256, bytes, hex_literal::hex, Bytes, Header, Log, LogData, Receipt, TxHash,
        TxNumber, TxType, B256, U256,
    };
    use reth_provider::ProviderResult;
    use std::{ops::RangeBounds, str::FromStr};

    use super::*;

    const FEERATE: FeeRate = FeeRate::from_sat_per_kwu(5 * 250);

    #[derive(Clone)]
    struct MockProvider {}

    impl MockProvider {
        fn new() -> Self {
            Self {}
        }

        fn receipt() -> Receipt {
            Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 0x1u64,
                logs: vec![Log::new_unchecked(
                    address!("0000000000000000000000000000000000000011"),
                    vec![
                        b256!("000000000000000000000000000000000000000000000000000000000000dead"),
                        b256!("000000000000000000000000000000000000000000000000000000000000beef"),
                    ],
                    bytes!("0100ff"),
                )],
                success: false,
            }
        }
    }

    impl ReceiptProvider for MockProvider {
        fn receipt(&self, _id: TxNumber) -> ProviderResult<Option<Receipt>> {
            Ok(Some(MockProvider::receipt()))
        }

        // return receipt with burn log
        fn receipt_by_hash(&self, _hash: TxHash) -> ProviderResult<Option<Receipt>> {
            // encoded values (amount, destination, version)
            let amount =
                ethabi::Token::Uint(ethabi::ethereum_types::U256::from(10_000_000_000_000_u64));
            let destination =
                ethabi::Token::String("mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh".to_string());
            let version = ethabi::Token::Bytes(vec![0]);
            let payload = ethabi::encode(&[amount, destination, version]);

            let log = Log {
                address: *MINT_CONTRACT_ADDRESS,
                data: LogData::new(
                    vec![
                        *BURN_TOPIC,
                        // msg.sender
                        B256::from(hex!(
                            "000000000000000000000000a65812bac44dadb79c3e4930dbd98d5a75376b2a"
                        )),
                    ],
                    Bytes::copy_from_slice(payload.as_slice()),
                )
                .unwrap(),
            };

            let mut receipt = MockProvider::receipt();
            receipt.logs = vec![log];

            Ok(Some(receipt))
        }

        fn receipts_by_block(
            &self,
            _block: BlockHashOrNumber,
        ) -> ProviderResult<Option<Vec<Receipt>>> {
            Ok(Some(vec![MockProvider::receipt()]))
        }

        fn receipts_by_tx_range(
            &self,
            _range: impl RangeBounds<TxNumber>,
        ) -> ProviderResult<Vec<Receipt>> {
            Ok(vec![MockProvider::receipt()])
        }
    }

    fn create_random_pegout_id() -> PegoutId {
        let mut rng = thread_rng();
        let mut pegout_id = [0u8; 36];
        rng.fill_bytes(&mut pegout_id);
        PegoutId::from_bytes(&pegout_id).unwrap()
    }

    // Util function to create a btc tx with random inputs and outputs as defined by fn params
    // copied over from btc-server so didn't have to expose mods
    fn create_tx(num_inputs: usize, address: &bitcoin::Address) -> Transaction {
        let txid = random_txid();

        let mut inputs = vec![];
        for i in 0..num_inputs {
            let op = OutPoint::new(txid, i as u32);
            inputs.push(TxIn {
                previous_output: op,
                script_sig: bitcoin::ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Default::default(),
            });
        }

        // Hardcoded one output
        let outputs = vec![bitcoin::TxOut {
            value: Amount::from_sat(1000),
            script_pubkey: address.script_pubkey(),
        }];
        Transaction {
            version: bitcoin::transaction::Version(2),
            lock_time: LockTime::ZERO,
            input: inputs,
            output: outputs,
        }
    }

    fn random_txid() -> Txid {
        let mut rng = thread_rng();
        let mut txid = [0u8; 32];
        rng.fill_bytes(&mut txid);
        Txid::from_slice(&txid).unwrap()
    }

    fn create_psbt(num_inputs: usize, address: &bitcoin::Address) -> Psbt {
        let tx = create_tx(num_inputs, address);

        let weight = tx.weight();
        let fee = FEERATE * weight;
        let input_needed = fee.to_sat() + tx.output.iter().map(|o| o.value.to_sat()).sum::<u64>();
        let value_per_input = input_needed / num_inputs as u64 + 1;

        let mut psbt = Psbt::from_unsigned_tx(tx).expect("valid psbt");
        for i in 0..num_inputs {
            psbt.inputs[i].witness_utxo = Some(bitcoin::TxOut {
                value: Amount::from_sat(value_per_input),
                script_pubkey: bitcoin::ScriptBuf::new(),
            });
        }
        psbt
    }

    #[test]
    fn generate_empty_merkel_root() {
        // generate merkel root with no utxos
        let root = generate_utxo_merkel_root(&[]).unwrap();
        assert_eq!(root, sha256::Hash::all_zeros());
    }

    #[test]
    fn generate_non_empty_merkel_root() {
        let mut rng = thread_rng();
        let mut utxos = vec![];
        // generate utxos
        for _ in 0..100 {
            let txid =
                bitcoin::Txid::from_slice(&rng.gen::<[u8; 32]>()).unwrap().to_byte_array().to_vec();
            let script_pubkey = rng.gen::<[u8; 32]>().to_vec();
            let vout = rng.gen_range(0..u32::MAX);
            let utxo = Utxo {
                outpoint: Some(client::OutPoint { txid: txid.clone(), vout }),
                output: Some(TxOut {
                    script_pubkey: Some(client::ScriptBuf { script: script_pubkey }),
                    value: rng.gen::<u64>(),
                }),
                eth_address: "0x0".to_string(),
            };
            utxos.push(utxo);
        }

        // generate merkel root with no utxos
        let root = generate_utxo_merkel_root(&utxos).unwrap();
        assert_ne!(root, sha256::Hash::all_zeros());

        // Root of the first 20 utxos
        let root_1 = generate_utxo_merkel_root(&utxos[0..20]).unwrap();
        assert_ne!(root_1, root);
        // TODO more assertions
        // Should try with out any eth address or other opti
    }

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
                    requests_root: Some(b256!(
                        "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"
                    )),
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
        // test happy path
        let precompiled_bytecode = String::from("60806040526004361061003f5760003560e01c80635fe03f45146100445780636f194dc914610066578063a5d0bb93146100b3578063a8de6d8c146100d6575b600080fd5b34801561005057600080fd5b5061006461005f366004610489565b6100fd565b005b34801561007257600080fd5b50610099610081366004610512565b60006020819052908152604090205463ffffffff1681565b60405163ffffffff90911681526020015b60405180910390f35b6100c66100c1366004610534565b610349565b60405190151581526020016100aa565b3480156100e257600080fd5b506100ef6402540be40081565b6040519081526020016100aa565b60005a6001600160a01b03881660009081526020819052604090205490915063ffffffff9081169086161161018b5760405162461bcd60e51b815260206004820152602960248201527f7573657220626974636f696e426c6f636b486569676874206e6565647320746f60448201526820696e63726561736560b81b60648201526084015b60405180910390fd5b6001600160a01b0387166000908152602081905260408120805463ffffffff191663ffffffff88161790553a60016101c46004876105b6565b61048560036107d3615208805a6101db908b6105d8565b6101e591906105f1565b6101ef91906105f1565b6101f991906105f1565b61020391906105f1565b61020d91906105f1565b61021791906105f1565b61022191906105d8565b61022b9190610604565b90508681111561027d5760405162461bcd60e51b815260206004820152601c60248201527f547820636f7374206578636565647320706567696e20616d6f756e74000000006044820152606401610182565b61028781886105d8565b6040519097506001600160a01b0389169088156108fc029089906000818181858888f193505050501580156102c0573d6000803e3d6000fd5b506040516001600160a01b0384169082156108fc029083906000818181858888f193505050501580156102f7573d6000803e3d6000fd5b50876001600160a01b03167f922344dc04648c0ce028ecdf9b2c9eed9a6794dbb47b777b54b0cfe069f128aa888888886040516103379493929190610644565b60405180910390a25050505050505050565b600061035c6402540be40061014a610604565b34116103d05760405162461bcd60e51b815260206004820152603860248201527f56616c7565206d7573742062652067726561746572207468616e20647573742060448201527f616d6f756e74206f662033333020736174732f764279746500000000000000006064820152608401610182565b336001600160a01b03167f17f87987da8ca71c697791dcfd190d07630cf17bf09c65c5a59b8277d9fe17153487878787604051610411959493929190610674565b60405180910390a2506001949350505050565b80356001600160a01b038116811461043b57600080fd5b919050565b60008083601f84011261045257600080fd5b50813567ffffffffffffffff81111561046a57600080fd5b60208301915083602082850101111561048257600080fd5b9250929050565b60008060008060008060a087890312156104a257600080fd5b6104ab87610424565b955060208701359450604087013563ffffffff811681146104cb57600080fd5b9350606087013567ffffffffffffffff8111156104e757600080fd5b6104f389828a01610440565b9094509250610506905060808801610424565b90509295509295509295565b60006020828403121561052457600080fd5b61052d82610424565b9392505050565b6000806000806040858703121561054a57600080fd5b843567ffffffffffffffff8082111561056257600080fd5b61056e88838901610440565b9096509450602087013591508082111561058757600080fd5b5061059487828801610440565b95989497509550505050565b634e487b7160e01b600052601160045260246000fd5b6000826105d357634e487b7160e01b600052601260045260246000fd5b500490565b818103818111156105eb576105eb6105a0565b92915050565b808201808211156105eb576105eb6105a0565b80820281158282048414176105eb576105eb6105a0565b81835281816020850137506000828201602090810191909152601f909101601f19169091010190565b84815263ffffffff8416602082015260606040820152600061066a60608301848661061b565b9695505050505050565b85815260606020820152600061068e60608301868861061b565b82810360408401526106a181858761061b565b9897505050505050505056fea2646970667358221220cf16442b31d8d5a64fc0a5e558f76e2e76039b54484fece01be27ffcf75ede6f64736f6c63430008150033");
        let deployed_bytecode = [
            96, 128, 96, 64, 82, 96, 4, 54, 16, 97, 0, 63, 87, 96, 0, 53, 96, 224, 28, 128, 99, 95,
            224, 63, 69, 20, 97, 0, 68, 87, 128, 99, 111, 25, 77, 201, 20, 97, 0, 102, 87, 128, 99,
            165, 208, 187, 147, 20, 97, 0, 179, 87, 128, 99, 168, 222, 109, 140, 20, 97, 0, 214,
            87, 91, 96, 0, 128, 253, 91, 52, 128, 21, 97, 0, 80, 87, 96, 0, 128, 253, 91, 80, 97,
            0, 100, 97, 0, 95, 54, 96, 4, 97, 4, 137, 86, 91, 97, 0, 253, 86, 91, 0, 91, 52, 128,
            21, 97, 0, 114, 87, 96, 0, 128, 253, 91, 80, 97, 0, 153, 97, 0, 129, 54, 96, 4, 97, 5,
            18, 86, 91, 96, 0, 96, 32, 129, 144, 82, 144, 129, 82, 96, 64, 144, 32, 84, 99, 255,
            255, 255, 255, 22, 129, 86, 91, 96, 64, 81, 99, 255, 255, 255, 255, 144, 145, 22, 129,
            82, 96, 32, 1, 91, 96, 64, 81, 128, 145, 3, 144, 243, 91, 97, 0, 198, 97, 0, 193, 54,
            96, 4, 97, 5, 52, 86, 91, 97, 3, 73, 86, 91, 96, 64, 81, 144, 21, 21, 129, 82, 96, 32,
            1, 97, 0, 170, 86, 91, 52, 128, 21, 97, 0, 226, 87, 96, 0, 128, 253, 91, 80, 97, 0,
            239, 100, 2, 84, 11, 228, 0, 129, 86, 91, 96, 64, 81, 144, 129, 82, 96, 32, 1, 97, 0,
            170, 86, 91, 96, 0, 90, 96, 1, 96, 1, 96, 160, 27, 3, 136, 22, 96, 0, 144, 129, 82, 96,
            32, 129, 144, 82, 96, 64, 144, 32, 84, 144, 145, 80, 99, 255, 255, 255, 255, 144, 129,
            22, 144, 134, 22, 17, 97, 1, 139, 87, 96, 64, 81, 98, 70, 27, 205, 96, 229, 27, 129,
            82, 96, 32, 96, 4, 130, 1, 82, 96, 41, 96, 36, 130, 1, 82, 127, 117, 115, 101, 114, 32,
            98, 105, 116, 99, 111, 105, 110, 66, 108, 111, 99, 107, 72, 101, 105, 103, 104, 116,
            32, 110, 101, 101, 100, 115, 32, 116, 111, 96, 68, 130, 1, 82, 104, 32, 105, 110, 99,
            114, 101, 97, 115, 101, 96, 184, 27, 96, 100, 130, 1, 82, 96, 132, 1, 91, 96, 64, 81,
            128, 145, 3, 144, 253, 91, 96, 1, 96, 1, 96, 160, 27, 3, 135, 22, 96, 0, 144, 129, 82,
            96, 32, 129, 144, 82, 96, 64, 129, 32, 128, 84, 99, 255, 255, 255, 255, 25, 22, 99,
            255, 255, 255, 255, 136, 22, 23, 144, 85, 58, 96, 1, 97, 1, 196, 96, 4, 135, 97, 5,
            182, 86, 91, 97, 4, 133, 96, 3, 97, 7, 211, 97, 82, 8, 128, 90, 97, 1, 219, 144, 139,
            97, 5, 216, 86, 91, 97, 1, 229, 145, 144, 97, 5, 241, 86, 91, 97, 1, 239, 145, 144, 97,
            5, 241, 86, 91, 97, 1, 249, 145, 144, 97, 5, 241, 86, 91, 97, 2, 3, 145, 144, 97, 5,
            241, 86, 91, 97, 2, 13, 145, 144, 97, 5, 241, 86, 91, 97, 2, 23, 145, 144, 97, 5, 241,
            86, 91, 97, 2, 33, 145, 144, 97, 5, 216, 86, 91, 97, 2, 43, 145, 144, 97, 6, 4, 86, 91,
            144, 80, 134, 129, 17, 21, 97, 2, 125, 87, 96, 64, 81, 98, 70, 27, 205, 96, 229, 27,
            129, 82, 96, 32, 96, 4, 130, 1, 82, 96, 28, 96, 36, 130, 1, 82, 127, 84, 120, 32, 99,
            111, 115, 116, 32, 101, 120, 99, 101, 101, 100, 115, 32, 112, 101, 103, 105, 110, 32,
            97, 109, 111, 117, 110, 116, 0, 0, 0, 0, 96, 68, 130, 1, 82, 96, 100, 1, 97, 1, 130,
            86, 91, 97, 2, 135, 129, 136, 97, 5, 216, 86, 91, 96, 64, 81, 144, 151, 80, 96, 1, 96,
            1, 96, 160, 27, 3, 137, 22, 144, 136, 21, 97, 8, 252, 2, 144, 137, 144, 96, 0, 129,
            129, 129, 133, 136, 136, 241, 147, 80, 80, 80, 80, 21, 128, 21, 97, 2, 192, 87, 61, 96,
            0, 128, 62, 61, 96, 0, 253, 91, 80, 96, 64, 81, 96, 1, 96, 1, 96, 160, 27, 3, 132, 22,
            144, 130, 21, 97, 8, 252, 2, 144, 131, 144, 96, 0, 129, 129, 129, 133, 136, 136, 241,
            147, 80, 80, 80, 80, 21, 128, 21, 97, 2, 247, 87, 61, 96, 0, 128, 62, 61, 96, 0, 253,
            91, 80, 135, 96, 1, 96, 1, 96, 160, 27, 3, 22, 127, 146, 35, 68, 220, 4, 100, 140, 12,
            224, 40, 236, 223, 155, 44, 158, 237, 154, 103, 148, 219, 180, 123, 119, 123, 84, 176,
            207, 224, 105, 241, 40, 170, 136, 136, 136, 136, 96, 64, 81, 97, 3, 55, 148, 147, 146,
            145, 144, 97, 6, 68, 86, 91, 96, 64, 81, 128, 145, 3, 144, 162, 80, 80, 80, 80, 80, 80,
            80, 80, 86, 91, 96, 0, 97, 3, 92, 100, 2, 84, 11, 228, 0, 97, 1, 74, 97, 6, 4, 86, 91,
            52, 17, 97, 3, 208, 87, 96, 64, 81, 98, 70, 27, 205, 96, 229, 27, 129, 82, 96, 32, 96,
            4, 130, 1, 82, 96, 56, 96, 36, 130, 1, 82, 127, 86, 97, 108, 117, 101, 32, 109, 117,
            115, 116, 32, 98, 101, 32, 103, 114, 101, 97, 116, 101, 114, 32, 116, 104, 97, 110, 32,
            100, 117, 115, 116, 32, 96, 68, 130, 1, 82, 127, 97, 109, 111, 117, 110, 116, 32, 111,
            102, 32, 51, 51, 48, 32, 115, 97, 116, 115, 47, 118, 66, 121, 116, 101, 0, 0, 0, 0, 0,
            0, 0, 0, 96, 100, 130, 1, 82, 96, 132, 1, 97, 1, 130, 86, 91, 51, 96, 1, 96, 1, 96,
            160, 27, 3, 22, 127, 23, 248, 121, 135, 218, 140, 167, 28, 105, 119, 145, 220, 253, 25,
            13, 7, 99, 12, 241, 123, 240, 156, 101, 197, 165, 155, 130, 119, 217, 254, 23, 21, 52,
            135, 135, 135, 135, 96, 64, 81, 97, 4, 17, 149, 148, 147, 146, 145, 144, 97, 6, 116,
            86, 91, 96, 64, 81, 128, 145, 3, 144, 162, 80, 96, 1, 148, 147, 80, 80, 80, 80, 86, 91,
            128, 53, 96, 1, 96, 1, 96, 160, 27, 3, 129, 22, 129, 20, 97, 4, 59, 87, 96, 0, 128,
            253, 91, 145, 144, 80, 86, 91, 96, 0, 128, 131, 96, 31, 132, 1, 18, 97, 4, 82, 87, 96,
            0, 128, 253, 91, 80, 129, 53, 103, 255, 255, 255, 255, 255, 255, 255, 255, 129, 17, 21,
            97, 4, 106, 87, 96, 0, 128, 253, 91, 96, 32, 131, 1, 145, 80, 131, 96, 32, 130, 133, 1,
            1, 17, 21, 97, 4, 130, 87, 96, 0, 128, 253, 91, 146, 80, 146, 144, 80, 86, 91, 96, 0,
            128, 96, 0, 128, 96, 0, 128, 96, 160, 135, 137, 3, 18, 21, 97, 4, 162, 87, 96, 0, 128,
            253, 91, 97, 4, 171, 135, 97, 4, 36, 86, 91, 149, 80, 96, 32, 135, 1, 53, 148, 80, 96,
            64, 135, 1, 53, 99, 255, 255, 255, 255, 129, 22, 129, 20, 97, 4, 203, 87, 96, 0, 128,
            253, 91, 147, 80, 96, 96, 135, 1, 53, 103, 255, 255, 255, 255, 255, 255, 255, 255, 129,
            17, 21, 97, 4, 231, 87, 96, 0, 128, 253, 91, 97, 4, 243, 137, 130, 138, 1, 97, 4, 64,
            86, 91, 144, 148, 80, 146, 80, 97, 5, 6, 144, 80, 96, 128, 136, 1, 97, 4, 36, 86, 91,
            144, 80, 146, 149, 80, 146, 149, 80, 146, 149, 86, 91, 96, 0, 96, 32, 130, 132, 3, 18,
            21, 97, 5, 36, 87, 96, 0, 128, 253, 91, 97, 5, 45, 130, 97, 4, 36, 86, 91, 147, 146,
            80, 80, 80, 86, 91, 96, 0, 128, 96, 0, 128, 96, 64, 133, 135, 3, 18, 21, 97, 5, 74, 87,
            96, 0, 128, 253, 91, 132, 53, 103, 255, 255, 255, 255, 255, 255, 255, 255, 128, 130,
            17, 21, 97, 5, 98, 87, 96, 0, 128, 253, 91, 97, 5, 110, 136, 131, 137, 1, 97, 4, 64,
            86, 91, 144, 150, 80, 148, 80, 96, 32, 135, 1, 53, 145, 80, 128, 130, 17, 21, 97, 5,
            135, 87, 96, 0, 128, 253, 91, 80, 97, 5, 148, 135, 130, 136, 1, 97, 4, 64, 86, 91, 149,
            152, 148, 151, 80, 149, 80, 80, 80, 80, 86, 91, 99, 78, 72, 123, 113, 96, 224, 27, 96,
            0, 82, 96, 17, 96, 4, 82, 96, 36, 96, 0, 253, 91, 96, 0, 130, 97, 5, 211, 87, 99, 78,
            72, 123, 113, 96, 224, 27, 96, 0, 82, 96, 18, 96, 4, 82, 96, 36, 96, 0, 253, 91, 80, 4,
            144, 86, 91, 129, 129, 3, 129, 129, 17, 21, 97, 5, 235, 87, 97, 5, 235, 97, 5, 160, 86,
            91, 146, 145, 80, 80, 86, 91, 128, 130, 1, 128, 130, 17, 21, 97, 5, 235, 87, 97, 5,
            235, 97, 5, 160, 86, 91, 128, 130, 2, 129, 21, 130, 130, 4, 132, 20, 23, 97, 5, 235,
            87, 97, 5, 235, 97, 5, 160, 86, 91, 129, 131, 82, 129, 129, 96, 32, 133, 1, 55, 80, 96,
            0, 130, 130, 1, 96, 32, 144, 129, 1, 145, 144, 145, 82, 96, 31, 144, 145, 1, 96, 31,
            25, 22, 144, 145, 1, 1, 144, 86, 91, 132, 129, 82, 99, 255, 255, 255, 255, 132, 22, 96,
            32, 130, 1, 82, 96, 96, 96, 64, 130, 1, 82, 96, 0, 97, 6, 106, 96, 96, 131, 1, 132,
            134, 97, 6, 27, 86, 91, 150, 149, 80, 80, 80, 80, 80, 80, 86, 91, 133, 129, 82, 96, 96,
            96, 32, 130, 1, 82, 96, 0, 97, 6, 142, 96, 96, 131, 1, 134, 136, 97, 6, 27, 86, 91,
            130, 129, 3, 96, 64, 132, 1, 82, 97, 6, 161, 129, 133, 135, 97, 6, 27, 86, 91, 152,
            151, 80, 80, 80, 80, 80, 80, 80, 80, 86, 254, 162, 100, 105, 112, 102, 115, 88, 34, 18,
            32, 207, 22, 68, 43, 49, 216, 213, 166, 79, 192, 165, 229, 88, 247, 110, 46, 118, 3,
            155, 84, 72, 79, 236, 224, 27, 226, 127, 252, 247, 94, 222, 111, 100, 115, 111, 108,
            99, 67, 0, 8, 21, 0, 51,
        ];

        assert!(is_known_minting_contract(precompiled_bytecode.clone(), &deployed_bytecode).is_ok());

        // test fail path
        let deployed_bytecode = "not known minting contract bytecode".as_bytes();
        assert!(is_known_minting_contract(precompiled_bytecode, deployed_bytecode).is_err());
    }

    #[test]
    fn extract_pegout_ids_should_return_pegout_ids() {
        let address = bitcoin::Address::from_str("mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh")
            .expect("valid address")
            .assume_checked();
        let pegout_id = create_random_pegout_id().as_bytes();
        let tx = create_tx(1, &address);
        let mut psbt = Psbt::from_unsigned_tx(tx).expect("psbt to be created");

        psbt.outputs[0].set_pegout_id(pegout_id);

        let result = extract_pegout_ids(&psbt);
        let expected = PegoutId::from(pegout_id);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0], expected);
    }

    #[test]
    fn validate_psbt_by_output_should_validate() {
        let value = bitcoin::Amount::from_sat(1000);
        let destination = bitcoin::Address::from_str("mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh")
            .expect("valid address")
            .assume_checked();
        let psbt = create_psbt(1, &destination);

        let result = validate_psbt_by_output(&psbt, &destination, value);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_psbt_by_output_should_fail_with_no_matching_value() {
        // value should be 1000
        let value = bitcoin::Amount::from_sat(1);
        let destination = bitcoin::Address::from_str("mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh")
            .expect("valid address")
            .assume_checked();
        let psbt = create_psbt(1, &destination);

        let result = validate_psbt_by_output(&psbt, &destination, value);
        assert!(result.is_err());
    }

    #[test]
    fn validate_psbt_by_output_should_fail_with_no_matching_destination() {
        let value = bitcoin::Amount::from_sat(1000);
        let destination = bitcoin::Address::from_str("mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh")
            .expect("valid address")
            .assume_checked();
        let psbt = create_psbt(1, &destination);

        let different_address = bitcoin::Address::from_str(
            "bcrt1pun3g8y2jjchy0834va959e0swmz8hc7gc2w4ure54ejzyekvp89scegtyh",
        )
        .expect("valid address")
        .assume_checked();

        let result = validate_psbt_by_output(&psbt, &different_address, value);
        assert!(result.is_err());
    }

    // fail paths are covered by above tests (ie no matching value, no matching destination)
    #[tokio::test]
    async fn validate_psbt_by_ids_should_validate() {
        let destination = bitcoin::Address::from_str("mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh")
            .expect("valid address")
            .assume_checked();
        let mut psbt = create_psbt(1, &destination);

        let pegout_id = PegoutId::new([0u8; 32], 0).as_bytes();
        psbt.outputs[0].set_pegout_id(pegout_id);

        let result =
            validate_psbt_by_ids(MockProvider::new(), bitcoin::Network::Regtest, &psbt).await;
        assert!(result.is_ok());
    }
}
