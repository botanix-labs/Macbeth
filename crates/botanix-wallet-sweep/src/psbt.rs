//! Emergency wallet sweep PSBT creation
//!
//! This module provides functionality to create PSBTs (Partially Signed Bitcoin Transactions)
//! for emergency wallet sweeps in the Botanix federation. Emergency sweeps collect all available
//! UTXOs and create a single transaction to move funds to a secure destination.
//!
//! The module supports both synchronous and asynchronous PSBT creation, with automatic
//! filtering of tracked UTXOs, proper fee calculation, and transaction size limits to ensure
//! reliable Bitcoin network propagation.
//!
//! ## Fee Calculation Compatibility
//!
//! The fee calculations in this module are IDENTICAL to those used in btc-server's coin selection
//! (bin/btc-server/src/wallet/util.rs and bin/btc-server/src/wallet/coin_selection.rs).
//! Both systems use the same weight constants and calculation methods to ensure perfect
//! consistency across the federation.

use bitcoin::{
    hashes::Hash, psbt::PsbtSighashType, sighash::TapSighashType, Amount, FeeRate, OutPoint, Psbt,
    ScriptBuf, TxOut, Weight,
};
use botanix_storage::models::WalletSweepSession;
use btc_server_client::{BtcServerExtendedApi, Empty};
use std::collections::HashSet;
use thiserror::Error;

// Transaction size limits for reliable Bitcoin network propagation
/// Maximum transaction weight for emergency sweeps (400,000 WU ≈ 100KB)
/// Uses standard relay limit to ensure propagation across all nodes while maximizing UTXO
/// throughput
pub const EMERGENCY_SWEEP_WEIGHT_LIMIT: u64 = 400_000;

/// Taproot keyspend input weight using IDENTICAL calculations to btc-server
/// - Base input: 36 bytes (outpoint) + 1 byte (scriptSig) + 4 bytes (sequence) = 164 WU
/// - Witness: TAPROOT_KEYSPEND_SIGHASH_DEFAULT_WEIGHT (65 WU) + witness item count (1 WU) = 66 WU
/// - Total: 230 WU per input (matches btc-server calculations exactly)
pub const TAPROOT_KEYSPEND_INPUT_WEIGHT: u64 = 230;

/// Base transaction weight for emergency sweep transactions
/// - Version (4) + locktime (4) + segwit flag/marker (2) + output count (1) + P2TR output (34) =
///   186 WU
/// - Input count encoding: 12 WU (supports up to 65535 inputs)
pub const EMERGENCY_SWEEP_BASE_WEIGHT: u64 = 186;

/// Maximum number of inputs for emergency sweep transactions
/// Calculated dynamically with 5% safety margin for reliable propagation
pub const MAX_EMERGENCY_SWEEP_INPUTS: usize = calculate_max_emergency_inputs();

/// Weight constants for precise fee calculation
///
/// NOTE: These calculations are IDENTICAL to btc-server's coin selection fee calculation
/// (bin/btc-server/src/wallet/util.rs::calculate_signed_tx_weight) and use the same constants:
///
/// - TAPROOT_KEYSPEND_SIGHASH_DEFAULT_WEIGHT = 65 WU (signature without sighash byte)
/// - per_input_witness_item_count = 1 WU
/// - Total per input: 66 WU
///
/// This ensures perfect compatibility and consistency across the federation.
///
/// TODO: Create a shared fee calculation crate to minimize reimplementation logic
/// between btc-server and botanix-wallet-sweep. Currently avoided due to circular
/// dependency (btc-server depends on botanix-wallet-sweep).
const TAPROOT_KEYSPEND_SIGHASH_DEFAULT_WEIGHT: Weight = Weight::from_wu(65);
const WITNESS_ITEM_COUNT_WEIGHT: Weight = Weight::from_wu(1);
const SEGWIT_FLAG_WEIGHT: Weight = Weight::from_wu(1);
const SEGWIT_MARKER_WEIGHT: Weight = Weight::from_wu(1);

/// Fee rate limits for emergency operations
const MAX_FEE_RATE_SAT_VB: u64 = 1000; // 1000 sat/vB = very high priority
const MIN_FEE_RATE_SAT_VB: u64 = 1; // 1 sat/vB = minimum relay fee

/// Maximum Bitcoin value in satoshis (21 million BTC)
const MAX_BITCOIN_VALUE_SATS: u64 = 21_000_000 * 100_000_000;

/// Calculate maximum emergency sweep inputs with safety margin
const fn calculate_max_emergency_inputs() -> usize {
    let available_weight = EMERGENCY_SWEEP_WEIGHT_LIMIT - EMERGENCY_SWEEP_BASE_WEIGHT;
    let theoretical_max = available_weight / TAPROOT_KEYSPEND_INPUT_WEIGHT;
    let safety_margin = theoretical_max / 20; // 5% margin for encoding variations
    (theoretical_max - safety_margin) as usize
}

/// Errors that can occur during emergency wallet sweep operations
#[derive(Debug, Error)]
pub enum SweepError {
    #[error("BTC server client error: {0}")]
    BtcServerClient(String),
    #[error("No UTXOs found in database")]
    NoUtxos,
    #[error("No available UTXOs - all are tracked or pending")]
    NoAvailableUtxos,
    #[error("Insufficient funds: fee {fee} exceeds total value {total_value}")]
    InsufficientFunds { fee: Amount, total_value: Amount },
    #[error("Invalid destination address: {0}")]
    InvalidDestination(String),
    #[error("Invalid fee rate: {fee_rate} sat/vB (must be between {min} and {max})")]
    InvalidFeeRate { fee_rate: u64, min: u64, max: u64 },
    #[error("Fee calculation overflow")]
    FeeCalculationOverflow,
    #[error("Weight calculation overflow")]
    WeightCalculationOverflow,
    #[error("Transaction too large: {actual_weight} WU exceeds limit of {max_weight} WU")]
    TransactionTooLarge { actual_weight: u64, max_weight: u64 },
    #[error("Too many UTXOs: {utxo_count} exceeds maximum of {max_utxos} for emergency sweep")]
    TooManyUtxos { utxo_count: usize, max_utxos: usize },
    #[error("Data parsing error: {0}")]
    DataParsing(String),
    #[error("Bitcoin serialization error: {0}")]
    BitcoinSerialization(#[from] bitcoin::consensus::encode::Error),
    #[error("Network parsing error: {0}")]
    NetworkParsing(String),
    #[error("Address validation error: {0}")]
    AddressValidation(String),
    #[error("Network mismatch: address is for {address_network} but destination network is {expected_network}")]
    NetworkMismatch { address_network: String, expected_network: String },
    #[error("Invalid UTXO: {0}")]
    InvalidUtxo(String),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error(
        "Output value too small: {value} sats is below dust threshold of {dust_threshold} sats"
    )]
    OutputBelowDustThreshold { value: u64, dust_threshold: u64 },
    #[error(
        "Invalid Bitcoin amount: {operation} value exceeds maximum supply of {max_supply} sats"
    )]
    InvalidBitcoinAmount { operation: String, max_supply: u64 },
    #[error("UTXO validation failed at index {index}: {details}")]
    UtxoValidationFailed { index: usize, details: String },
    #[error("Weight calculation failed: {details}")]
    WeightCalculationFailed { details: String },
}

/// UTXO version used in the Botanix federation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UtxoVersion {
    /// Version 1 UTXOs use taproot key spend path
    V1 = 0,
}

impl Default for UtxoVersion {
    fn default() -> Self {
        Self::V1
    }
}

/// Internal representation of a UTXO input for PSBT creation
#[derive(Debug, Clone)]
struct SweepInput {
    outpoint: OutPoint,
    output: TxOut,
    eth_address: Option<[u8; 20]>,
    version: UtxoVersion,
}

/// Creates an emergency wallet sweep PSBT by fetching all UTXOs from btc-server
///
/// This function retrieves all available UTXOs and tracked transactions from the database,
/// then creates a comprehensive emergency sweep PSBT with proper size limits and fee calculation.
///
/// # Arguments
/// * `request` - The wallet sweep request containing destination and fee information
/// * `client` - Mutable reference to the btc-server client for database access
///
/// # Returns
/// A PSBT that sweeps all available UTXOs to the specified destination address
pub async fn create_psbt_async(
    session: WalletSweepSession,
    client: &mut impl BtcServerExtendedApi,
) -> Result<Psbt, SweepError> {
    tracing::warn!("EMERGENCY: Starting wallet sweep PSBT creation");

    // Fetch all UTXOs and tracked transactions from database
    let utxos_response = client
        .get_all_utxos(Empty {})
        .await
        .map_err(|e| SweepError::BtcServerClient(format!("Failed to get UTXOs: {}", e)))?;
    let tracked_txs_response = client.get_tracked_txs(Empty {}).await.map_err(|e| {
        SweepError::BtcServerClient(format!("Failed to get tracked transactions: {}", e))
    })?;

    let utxos = utxos_response.utxos;
    if utxos.is_empty() {
        return Err(SweepError::NoUtxos);
    }

    let tracked_inputs = extract_tracked_outpoints(&tracked_txs_response.tracked_txs);

    tracing::warn!(
        "EMERGENCY: Processing {} UTXOs, excluding {} tracked inputs",
        utxos.len(),
        tracked_inputs.len()
    );

    create_psbt_from_utxos(session, utxos, tracked_inputs)
}

/// Creates an emergency wallet sweep PSBT from provided UTXO data
///
/// This function processes the provided UTXOs, applies size limits, calculates fees,
/// and creates a properly formatted PSBT for emergency wallet sweeps.
///
/// # Arguments
/// * `request` - The wallet sweep request containing destination and fee information
/// * `available_utxos` - Vector of UTXOs available for spending
/// * `tracked_inputs` - Set of outpoints currently being tracked/spent
///
/// # Returns
/// A PSBT that sweeps the provided UTXOs to the specified destination address
pub fn create_psbt_from_utxos(
    session: WalletSweepSession,
    available_utxos: Vec<btc_server_client::Utxo>,
    tracked_inputs: HashSet<OutPoint>,
) -> Result<Psbt, SweepError> {
    tracing::warn!("EMERGENCY: Creating sweep PSBT from {} UTXOs", available_utxos.len());

    if available_utxos.is_empty() {
        return Err(SweepError::NoUtxos);
    }

    // Validate session parameters first (before moving any fields)
    validate_session_parameters(&session)?;

    // Extract network from address instead of using separate field
    // TODO: Remove destination_network field from WalletSweepRequest in future commit
    let destination_network = extract_network_from_address(&session.bitcoin_destination_address)?;
    let destination_address = session
        .bitcoin_destination_address
        .require_network(destination_network)
        .map_err(|e| SweepError::AddressValidation(e.to_string()))?;

    // Network validation (TODO: integrate with node configuration)
    validate_network_consistency(destination_network)?;

    let fee_rate = FeeRate::from_sat_per_vb(session.fee_rate_sat_vb).ok_or_else(|| {
        SweepError::InvalidFeeRate {
            fee_rate: session.fee_rate_sat_vb,
            min: MIN_FEE_RATE_SAT_VB,
            max: MAX_FEE_RATE_SAT_VB,
        }
    })?;

    // Filter and prepare UTXOs for spending
    let spendable_utxos = filter_and_sort_utxos(available_utxos, &tracked_inputs)?;
    if spendable_utxos.is_empty() {
        return Err(SweepError::NoAvailableUtxos);
    }

    let limited_utxos = apply_size_limits(spendable_utxos)?;

    // Check that we still have UTXOs after applying size limits
    if limited_utxos.is_empty() {
        return Err(SweepError::NoAvailableUtxos);
    }

    let total_input_value = calculate_total_value(&limited_utxos)?;

    // Check that truncated UTXOs have non-zero total value
    if total_input_value == Amount::ZERO {
        return Err(SweepError::InsufficientFunds {
            fee: Amount::ZERO,
            total_value: total_input_value,
        });
    }

    tracing::warn!(
        "EMERGENCY: Selected {} UTXOs with total value {} sats",
        limited_utxos.len(),
        total_input_value.to_sat()
    );

    // Convert to internal format and calculate fees
    let inputs = convert_to_sweep_inputs(limited_utxos)?;
    validate_bitcoin_amount_bounds(total_input_value, "total input value")?;

    // Calculate fee using preliminary PSBT
    let preliminary_output =
        TxOut { script_pubkey: destination_address.script_pubkey(), value: total_input_value };
    let preliminary_psbt = build_preliminary_psbt(&inputs, &preliminary_output);
    let fee = calculate_transaction_fee(&preliminary_psbt, fee_rate)?;

    // Create final output with fee subtracted
    let final_value = total_input_value
        .checked_sub(fee)
        .ok_or_else(|| SweepError::InsufficientFunds { fee, total_value: total_input_value })?;

    validate_dust_threshold(final_value, destination_address.script_pubkey())?;
    validate_bitcoin_amount_bounds(final_value, "final output value")?;

    let final_output =
        TxOut { script_pubkey: destination_address.script_pubkey(), value: final_value };

    // Build and verify final PSBT
    let psbt = build_sweep_psbt(inputs, final_output)?;
    verify_transaction_size_limits(&psbt)?;

    tracing::error!(
        "EMERGENCY SWEEP COMPLETED: {} inputs, fee: {} sats, output: {} sats, weight: {} WU",
        psbt.inputs.len(),
        fee.to_sat(),
        final_value.to_sat(),
        calculate_transaction_weight(&psbt)?
    );

    Ok(psbt)
}

/// Filters UTXOs by availability and sorts them by value (largest first) for optimal recovery
fn filter_and_sort_utxos(
    utxos: Vec<btc_server_client::Utxo>,
    tracked_inputs: &HashSet<OutPoint>,
) -> Result<Vec<btc_server_client::Utxo>, SweepError> {
    let mut spendable_utxos = filter_spendable_utxos(utxos, tracked_inputs)?;

    // TODO: Sort by effective value instead of raw value for optimal UTXO selection
    // Effective value = utxo_value - fee_to_spend_utxo, which accounts for the cost of including
    // each UTXO in the transaction. This would be more efficient than sorting by raw value,
    // especially for emergency sweeps where we want to maximize recovered funds.
    //
    // Sort by value descending (largest first), then by outpoint for deterministic ordering
    spendable_utxos.sort_by(|a, b| {
        let value_a = a.output.as_ref().map(|o| o.value).unwrap_or(0);
        let value_b = b.output.as_ref().map(|o| o.value).unwrap_or(0);

        // Primary sort: value descending
        let value_cmp = value_b.cmp(&value_a);
        if value_cmp != std::cmp::Ordering::Equal {
            return value_cmp;
        }

        // Secondary sort: lexicographic outpoint ordering for deterministic results
        match (a.outpoint.as_ref(), b.outpoint.as_ref()) {
            (Some(op_a), Some(op_b)) => {
                let txid_cmp = op_a.txid.cmp(&op_b.txid);
                if txid_cmp != std::cmp::Ordering::Equal {
                    txid_cmp
                } else {
                    op_a.vout.cmp(&op_b.vout)
                }
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });

    Ok(spendable_utxos)
}

/// Applies transaction size limits to prevent oversized transactions
fn apply_size_limits(
    mut utxos: Vec<btc_server_client::Utxo>,
) -> Result<Vec<btc_server_client::Utxo>, SweepError> {
    // TODO: When truncating UTXOs, consider effective value to select the most profitable ones
    // Currently we truncate after sorting by raw value, but effective value would be more optimal
    if utxos.len() > MAX_EMERGENCY_SWEEP_INPUTS {
        tracing::warn!(
            "UTXO count {} exceeds maximum {}, limiting to top {} by value",
            utxos.len(),
            MAX_EMERGENCY_SWEEP_INPUTS,
            MAX_EMERGENCY_SWEEP_INPUTS
        );
        utxos.truncate(MAX_EMERGENCY_SWEEP_INPUTS);
    }

    // Double-check against weight-based limits
    let available_weight = EMERGENCY_SWEEP_WEIGHT_LIMIT.saturating_sub(EMERGENCY_SWEEP_BASE_WEIGHT);
    if available_weight < TAPROOT_KEYSPEND_INPUT_WEIGHT {
        return Err(SweepError::TooManyUtxos { utxo_count: utxos.len(), max_utxos: 0 });
    }

    let max_inputs_by_weight = (available_weight / TAPROOT_KEYSPEND_INPUT_WEIGHT) as usize;
    let final_count = std::cmp::min(utxos.len(), max_inputs_by_weight);

    utxos.truncate(final_count);

    tracing::info!(
        "Size limits applied: using {} UTXOs (estimated weight: {} WU)",
        utxos.len(),
        EMERGENCY_SWEEP_BASE_WEIGHT + (utxos.len() as u64 * TAPROOT_KEYSPEND_INPUT_WEIGHT)
    );

    Ok(utxos)
}

/// Verifies that a PSBT meets emergency sweep transaction size limits
fn verify_transaction_size_limits(psbt: &Psbt) -> Result<(), SweepError> {
    let weight = calculate_transaction_weight(psbt)?;

    if weight > EMERGENCY_SWEEP_WEIGHT_LIMIT {
        return Err(SweepError::TransactionTooLarge {
            actual_weight: weight,
            max_weight: EMERGENCY_SWEEP_WEIGHT_LIMIT,
        });
    }

    Ok(())
}

/// Calculates the total weight of a PSBT transaction including signatures
///
/// This function uses IDENTICAL weight calculations to btc-server's utility function
/// (bin/btc-server/src/wallet/util.rs::calculate_signed_tx_weight), ensuring
/// perfect consistency with the federation's standard transaction weight calculations.
fn calculate_transaction_weight(psbt: &Psbt) -> Result<u64, SweepError> {
    let base_weight = psbt.unsigned_tx.weight();
    let num_inputs = psbt.inputs.len() as u64;

    let signature_weight = (TAPROOT_KEYSPEND_SIGHASH_DEFAULT_WEIGHT + WITNESS_ITEM_COUNT_WEIGHT)
        .checked_mul(num_inputs)
        .ok_or(SweepError::WeightCalculationOverflow)?;

    let total_weight = base_weight
        .checked_add(signature_weight)
        .and_then(|w| w.checked_add(SEGWIT_FLAG_WEIGHT))
        .and_then(|w| w.checked_add(SEGWIT_MARKER_WEIGHT))
        .ok_or(SweepError::WeightCalculationOverflow)?;

    Ok(total_weight.to_wu())
}

/// Extracts outpoints from tracked transactions to identify UTXOs already being spent
fn extract_tracked_outpoints(tracked_txs: &[btc_server_client::TrackedTx]) -> HashSet<OutPoint> {
    tracked_txs
        .iter()
        .filter_map(|tracked_tx| tracked_tx.tx.as_ref())
        .flat_map(|tx| &tx.input)
        .filter_map(|input| {
            let outpoint = input.previous_outpoint.as_ref()?;
            let txid = bitcoin::Txid::from_slice(&outpoint.txid).ok()?;
            Some(OutPoint { txid, vout: outpoint.vout })
        })
        .collect()
}

/// Extracts the network from a Bitcoin address
/// This eliminates the need for a separate destination_network field
fn extract_network_from_address(
    // TODO: You shouldn't accept ref and then clone inside (hidden clone). You need to accept
    // owned value so you make it implicit
    address: &bitcoin::Address<bitcoin::address::NetworkUnchecked>,
) -> Result<bitcoin::Network, SweepError> {
    // Try parsing with different networks - this is the most reliable approach
    // since it uses the bitcoin library's own validation logic
    for network in [
        bitcoin::Network::Bitcoin,
        bitcoin::Network::Testnet,
        bitcoin::Network::Signet,
        bitcoin::Network::Regtest,
    ] {
        // TODO: is_valid_for_network ?
        if let Ok(_) = address.clone().require_network(network) {
            return Ok(network);
        }
    }

    Err(SweepError::AddressValidation(
        format!("Unable to determine network from address (address failed validation for all known networks)")
    ))
}

/// Validates network consistency (TODO: integrate with node configuration)
fn validate_network_consistency(destination_network: bitcoin::Network) -> Result<(), SweepError> {
    // TODO: Integrate with node configuration to get the actual expected network
    // For now, we implement basic validation rules
    match destination_network {
        bitcoin::Network::Bitcoin => {
            // Mainnet destinations are valid for production federations
        }
        bitcoin::Network::Testnet | bitcoin::Network::Signet | bitcoin::Network::Regtest => {
            // Non-mainnet destinations are valid for testing environments
        }
        _ => {
            return Err(SweepError::NetworkMismatch {
                address_network: destination_network.to_string(),
                expected_network: "bitcoin mainnet or known testnet".to_string(),
            });
        }
    }

    Ok(())
}

// TODO: We should validate request before we accpet and on btc server side
/// Validates session parameters for emergency wallet sweep
fn validate_session_parameters(session: &WalletSweepSession) -> Result<(), SweepError> {
    // Validate fee rate bounds
    if session.fee_rate_sat_vb == 0 ||
        session.fee_rate_sat_vb < MIN_FEE_RATE_SAT_VB ||
        session.fee_rate_sat_vb > MAX_FEE_RATE_SAT_VB
    {
        return Err(SweepError::InvalidFeeRate {
            fee_rate: session.fee_rate_sat_vb,
            min: MIN_FEE_RATE_SAT_VB,
            max: MAX_FEE_RATE_SAT_VB,
        });
    }

    // Validate fee rate construction
    if FeeRate::from_sat_per_vb(session.fee_rate_sat_vb).is_none() {
        return Err(SweepError::InvalidFeeRate {
            fee_rate: session.fee_rate_sat_vb,
            min: MIN_FEE_RATE_SAT_VB,
            max: MAX_FEE_RATE_SAT_VB,
        });
    }

    Ok(())
}

/// Validates dust threshold based on the script_pubkey
fn validate_dust_threshold(value: Amount, script_pubkey: ScriptBuf) -> Result<(), SweepError> {
    let dust_threshold = script_pubkey.minimal_non_dust();

    if value < dust_threshold {
        return Err(SweepError::OutputBelowDustThreshold {
            value: value.to_sat(),
            dust_threshold: dust_threshold.to_sat(),
        });
    }
    Ok(())
}

/// Validates Bitcoin amount bounds to ensure values don't exceed protocol limits
fn validate_bitcoin_amount_bounds(value: Amount, operation: &str) -> Result<(), SweepError> {
    if value.to_sat() > MAX_BITCOIN_VALUE_SATS {
        return Err(SweepError::InvalidBitcoinAmount {
            operation: operation.to_string(),
            max_supply: MAX_BITCOIN_VALUE_SATS,
        });
    }
    Ok(())
}

/// Validates UTXO maturity for spending (TODO: implement bitcoind integration)
fn validate_utxo_maturity(outpoint: &OutPoint) -> Result<(), SweepError> {
    // TODO: Implement actual maturity validation by querying bitcoind
    // For now, we assume all UTXOs in the database are mature enough to spend
    tracing::debug!(
        "UTXO maturity validation for {} - assuming mature (TODO: implement bitcoind check)",
        outpoint
    );
    Ok(())
}

/// Filters UTXOs to only include those that aren't being tracked/spent and validates them
fn filter_spendable_utxos(
    utxos: Vec<btc_server_client::Utxo>,
    tracked_inputs: &HashSet<OutPoint>,
) -> Result<Vec<btc_server_client::Utxo>, SweepError> {
    let mut valid_utxos = Vec::new();

    for (index, utxo) in utxos.into_iter().enumerate() {
        // Extract and validate UTXO components
        let outpoint_proto = utxo.outpoint.as_ref().ok_or_else(|| {
            SweepError::InvalidUtxo(format!("UTXO at index {} missing outpoint", index))
        })?;
        let output_proto = utxo.output.as_ref().ok_or_else(|| {
            SweepError::InvalidUtxo(format!("UTXO at index {} missing output", index))
        })?;
        let script_proto = output_proto.script_pubkey.as_ref().ok_or_else(|| {
            SweepError::InvalidUtxo(format!("UTXO at index {} missing script_pubkey", index))
        })?;

        let txid = bitcoin::Txid::from_slice(&outpoint_proto.txid).map_err(|_| {
            SweepError::InvalidUtxo(format!("UTXO at index {} has invalid txid format", index))
        })?;
        let outpoint = OutPoint { txid, vout: outpoint_proto.vout };

        // Validate UTXO maturity and properties
        validate_utxo_maturity(&outpoint)?;

        let script = ScriptBuf::from_bytes(script_proto.script.clone());

        // Validate taproot script (emergency sweeps only support taproot)
        if !script.is_p2tr() {
            return Err(SweepError::InvalidUtxo(format!(
                "UTXO at index {} ({}) is not taproot (emergency sweep only supports taproot)",
                index, outpoint
            )));
        }

        const P2TR_SCRIPT_LENGTH: usize = 34; // OP_1 + 32-byte key
        if script.len() != P2TR_SCRIPT_LENGTH {
            return Err(SweepError::InvalidUtxo(format!(
                "UTXO at index {} ({}) has invalid P2TR script length: {} bytes (expected {})",
                index,
                outpoint,
                script.len(),
                P2TR_SCRIPT_LENGTH
            )));
        }

        // Validate non-zero value
        if output_proto.value == 0 {
            return Err(SweepError::InvalidUtxo(format!(
                "UTXO at index {} ({}) has zero value",
                index, outpoint
            )));
        }

        // Warn about unusually high vout values
        const REASONABLE_VOUT_LIMIT: u32 = 10_000;
        if outpoint_proto.vout > REASONABLE_VOUT_LIMIT {
            tracing::warn!(
                "UTXO at index {} ({}) has unusually high vout: {} (possible data corruption)",
                index,
                outpoint,
                outpoint_proto.vout
            );
        }

        // Include UTXO if not already being tracked/spent
        if !tracked_inputs.contains(&outpoint) {
            valid_utxos.push(utxo);
        }
    }

    Ok(valid_utxos)
}

/// Calculates the total value of UTXOs with overflow protection
fn calculate_total_value(utxos: &[btc_server_client::Utxo]) -> Result<Amount, SweepError> {
    let mut total = Amount::ZERO;

    for (index, utxo) in utxos.iter().enumerate() {
        let utxo_value = utxo
            .output
            .as_ref()
            .map(|output| Amount::from_sat(output.value))
            .unwrap_or(Amount::ZERO);

        // Validate individual UTXO value doesn't exceed Bitcoin limits
        validate_bitcoin_amount_bounds(utxo_value, &format!("UTXO {} value", index))?;

        // Use checked addition to prevent overflow
        total = total.checked_add(utxo_value).ok_or_else(|| SweepError::InvalidBitcoinAmount {
            operation: format!("sum of UTXO values (overflow at UTXO index {})", index),
            max_supply: MAX_BITCOIN_VALUE_SATS,
        })?;

        // Additional check against Bitcoin's total supply limit
        if total.to_sat() > MAX_BITCOIN_VALUE_SATS {
            return Err(SweepError::InvalidBitcoinAmount {
                operation: format!(
                    "sum of UTXO values ({} sats exceeds maximum Bitcoin supply)",
                    total.to_sat()
                ),
                max_supply: MAX_BITCOIN_VALUE_SATS,
            });
        }
    }

    Ok(total)
}

/// Converts btc-server UTXOs to internal SweepInput format
fn convert_to_sweep_inputs(
    utxos: Vec<btc_server_client::Utxo>,
) -> Result<Vec<SweepInput>, SweepError> {
    utxos
        .iter()
        .enumerate()
        .map(|(i, utxo)| {
            let outpoint_proto = utxo
                .outpoint
                .as_ref()
                .ok_or_else(|| SweepError::DataParsing(format!("UTXO {} missing outpoint", i)))?;
            let output_proto = utxo
                .output
                .as_ref()
                .ok_or_else(|| SweepError::DataParsing(format!("UTXO {} missing output", i)))?;
            let script_proto = output_proto.script_pubkey.as_ref().ok_or_else(|| {
                SweepError::DataParsing(format!("UTXO {} missing script_pubkey", i))
            })?;

            let txid = bitcoin::Txid::from_slice(&outpoint_proto.txid)
                .map_err(|_| SweepError::DataParsing("Invalid txid".to_string()))?;
            let outpoint = OutPoint { txid, vout: outpoint_proto.vout };

            let value = Amount::from_sat(output_proto.value);
            validate_bitcoin_amount_bounds(value, &format!("UTXO {} value", i))?;

            let output =
                TxOut { value, script_pubkey: ScriptBuf::from_bytes(script_proto.script.clone()) };

            let eth_address = if utxo.eth_address.is_empty() {
                None
            } else {
                hex::decode(&utxo.eth_address).ok().and_then(|bytes| bytes.try_into().ok())
            };

            Ok(SweepInput { outpoint, output, eth_address, version: UtxoVersion::V1 })
        })
        .collect()
}

/// Creates a preliminary PSBT for fee calculation
fn build_preliminary_psbt(inputs: &[SweepInput], output: &TxOut) -> Psbt {
    let tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::locktime::absolute::LockTime::ZERO,
        input: inputs
            .iter()
            .map(|input| bitcoin::TxIn {
                previous_output: input.outpoint,
                sequence: bitcoin::Sequence::ENABLE_RBF_NO_LOCKTIME,
                script_sig: bitcoin::ScriptBuf::new(),
                witness: Default::default(),
            })
            .collect(),
        output: vec![output.clone()],
    };

    let mut psbt = Psbt::from_unsigned_tx(tx).expect("transaction is unsigned");

    // Add witness UTXOs for fee calculation
    for (psbt_input, sweep_input) in psbt.inputs.iter_mut().zip(inputs.iter()) {
        psbt_input.witness_utxo = Some(sweep_input.output.clone());
    }

    psbt
}

/// Calculates transaction fee using accurate weight estimation
///
/// This function uses IDENTICAL fee calculations to btc-server's coin selection
/// (bin/btc-server/src/wallet/coin_selection.rs::calculate_required_fee), ensuring
/// perfect consistency across the federation for emergency sweep operations.
fn calculate_transaction_fee(psbt: &Psbt, fee_rate: FeeRate) -> Result<Amount, SweepError> {
    let base_weight = psbt.unsigned_tx.weight();

    // Verify all inputs are taproot
    for (i, input) in psbt.inputs.iter().enumerate() {
        let witness_utxo = input
            .witness_utxo
            .as_ref()
            .ok_or_else(|| SweepError::DataParsing(format!("Input {} missing witness UTXO", i)))?;

        if !witness_utxo.script_pubkey.is_p2tr() {
            return Err(SweepError::DataParsing(format!(
                "Input {} is not taproot (emergency sweep only supports taproot)",
                i
            )));
        }
    }

    // Calculate total weight including signatures
    let num_inputs = psbt.inputs.len() as u64;
    let total_signature_weight = (TAPROOT_KEYSPEND_SIGHASH_DEFAULT_WEIGHT +
        WITNESS_ITEM_COUNT_WEIGHT)
        .checked_mul(num_inputs)
        .ok_or(SweepError::WeightCalculationOverflow)?;

    let total_weight = base_weight
        .checked_add(total_signature_weight)
        .and_then(|w| w.checked_add(SEGWIT_FLAG_WEIGHT))
        .and_then(|w| w.checked_add(SEGWIT_MARKER_WEIGHT))
        .ok_or(SweepError::WeightCalculationOverflow)?;

    fee_rate.fee_wu(total_weight).ok_or(SweepError::FeeCalculationOverflow)
}

// PSBT proprietary extension constants for metadata storage
const ETH_ADDRESS_KEY_TYPE: u8 = 1;
const UTXO_VERSION_KEY_TYPE: u8 = 4;

lazy_static::lazy_static! {
    static ref PROP_KEY_PREFIX: &'static [u8] = b"btx";

    static ref ETH_ADDRESS_KEY: bitcoin::psbt::raw::ProprietaryKey = bitcoin::psbt::raw::ProprietaryKey {
        prefix: PROP_KEY_PREFIX.to_vec(),
        subtype: ETH_ADDRESS_KEY_TYPE,
        key: Vec::new(),
    };

    static ref UTXO_VERSION_KEY: bitcoin::psbt::raw::ProprietaryKey = bitcoin::psbt::raw::ProprietaryKey {
        prefix: PROP_KEY_PREFIX.to_vec(),
        subtype: UTXO_VERSION_KEY_TYPE,
        key: Vec::new(),
    };
}

/// Extension trait for adding emergency sweep metadata to PSBT inputs
trait SweepPsbtInputExt {
    fn set_eth_address(&mut self, eth_address: [u8; 20]);
    fn set_utxo_version(&mut self, version: u32);
}

impl SweepPsbtInputExt for bitcoin::psbt::Input {
    fn set_eth_address(&mut self, eth_address: [u8; 20]) {
        self.proprietary.insert(ETH_ADDRESS_KEY.clone(), eth_address.to_vec());
    }

    fn set_utxo_version(&mut self, version: u32) {
        self.proprietary.insert(UTXO_VERSION_KEY.clone(), version.to_le_bytes().to_vec());
    }
}

/// Builds the final emergency sweep PSBT with complete metadata
fn build_sweep_psbt(inputs: Vec<SweepInput>, output: TxOut) -> Result<Psbt, SweepError> {
    let tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::locktime::absolute::LockTime::ZERO,
        input: inputs
            .iter()
            .map(|input| bitcoin::TxIn {
                previous_output: input.outpoint,
                sequence: bitcoin::Sequence::ENABLE_RBF_NO_LOCKTIME,
                script_sig: bitcoin::ScriptBuf::new(),
                witness: Default::default(),
            })
            .collect(),
        output: vec![output],
    };

    let mut psbt = Psbt::from_unsigned_tx(tx).expect("transaction is unsigned");

    // Add complete input metadata for FROST signing
    for (psbt_input, sweep_input) in psbt.inputs.iter_mut().zip(inputs.iter()) {
        // Required for signing
        psbt_input.witness_utxo = Some(sweep_input.output.clone());
        psbt_input.sighash_type = Some(PsbtSighashType::from(TapSighashType::Default));

        // Add proprietary metadata
        if let Some(eth_addr) = sweep_input.eth_address {
            psbt_input.set_eth_address(eth_addr);
        }
        psbt_input.set_utxo_version(sweep_input.version as u32);
    }

    Ok(psbt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{absolute::LockTime, transaction::Version, Address, Network, Txid};
    use std::str::FromStr;
    use crate::request::WalletSweepRequest;

    // Test helper functions
    #[allow(dead_code)]
    fn create_test_request() -> WalletSweepRequest {
        // Create a valid 64-byte coordinator signature for testing
        let mut coordinator_signature = vec![0u8; 64];
        for i in 0..64 {
            coordinator_signature[i] = (i % 256) as u8;
        }

        WalletSweepRequest {
            coordinator_id: 1,
            coordinator_signature,
            destination_network: Network::Bitcoin.to_string(),
            destination_address: Address::from_str("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
                .unwrap()
                .as_unchecked()
                .clone(),
            fee_rate_sat_vb: 10,
            created_at: 1234567890,
        }
    }

    fn create_test_session() -> WalletSweepSession {
        // Create a valid 64-byte coordinator signature for testing
        let mut coordinator_signature = vec![0u8; 64];
        for i in 0..64 {
            coordinator_signature[i] = (i % 256) as u8;
        }

        WalletSweepSession {
            bitcoin_network: Network::Bitcoin,
            bitcoin_destination_address: Address::from_str(
                "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
            )
            .unwrap()
            .as_unchecked()
            .clone(),
            fee_rate_sat_vb: 10,
            created_at: 1234567890,
        }
    }

    fn create_test_utxo(
        value_sats: u64,
        vout: u32,
        eth_address: Option<[u8; 20]>,
    ) -> btc_server_client::Utxo {
        use rand::{thread_rng, RngCore};
        let mut rng = thread_rng();
        let mut txid_bytes = [0u8; 32];
        rng.fill_bytes(&mut txid_bytes);

        // Create realistic P2TR script
        let mut script_data = vec![0x51, 0x20]; // OP_1 + 32 byte push
        let mut pubkey_hash = [0u8; 32];
        rng.fill_bytes(&mut pubkey_hash);
        script_data.extend_from_slice(&pubkey_hash);

        btc_server_client::Utxo {
            outpoint: Some(btc_server_client::OutPoint { txid: txid_bytes.to_vec(), vout }),
            output: Some(btc_server_client::TxOut {
                value: value_sats,
                script_pubkey: Some(btc_server_client::ScriptBuf { script: script_data }),
            }),
            eth_address: eth_address.map(|addr| hex::encode(addr)).unwrap_or_default(),
        }
    }

    fn create_test_input(value_sats: u64, vout: u32, eth_address: Option<[u8; 20]>) -> SweepInput {
        let txid =
            Txid::from_str("1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef")
                .unwrap();
        let outpoint = OutPoint::new(txid, vout);

        // Create P2TR script
        let script_bytes =
            vec![0x51, 0x20].into_iter().chain(std::iter::repeat(0x00).take(32)).collect();

        SweepInput {
            outpoint,
            output: TxOut {
                value: Amount::from_sat(value_sats),
                script_pubkey: ScriptBuf::from_bytes(script_bytes),
            },
            eth_address,
            version: UtxoVersion::V1,
        }
    }

    fn create_tracked_transaction(input_outpoints: Vec<OutPoint>) -> btc_server_client::TrackedTx {
        let tx_inputs: Vec<btc_server_client::TxIn> = input_outpoints
            .into_iter()
            .map(|outpoint| btc_server_client::TxIn {
                previous_outpoint: Some(btc_server_client::OutPoint {
                    txid: outpoint.txid.to_byte_array().to_vec(),
                    vout: outpoint.vout,
                }),
                script_sig: Some(btc_server_client::ScriptBuf { script: vec![] }),
                sequence: 0xFFFFFFFF,
                witness: vec![],
            })
            .collect();

        btc_server_client::TrackedTx {
            txid: vec![0u8; 32],
            tx: Some(btc_server_client::Transaction {
                version: 2,
                lock_time: 0,
                input: tx_inputs,
                output: vec![],
            }),
            pegout_idxs: vec![],
            pegout_requests: vec![],
            change_idxs: vec![],
            created: None,
        }
    }

    // Essential tests - core functionality and error handling
    #[test]
    fn test_empty_utxos() {
        let session = create_test_session();
        let result = create_psbt_from_utxos(session, vec![], HashSet::new());
        assert!(matches!(result.unwrap_err(), SweepError::NoUtxos));
    }

    #[test]
    fn test_all_utxos_tracked() {
        let session = create_test_session();
        let utxo = create_test_utxo(100_000, 0, None);
        let outpoint = OutPoint {
            txid: Txid::from_slice(&utxo.outpoint.as_ref().unwrap().txid).unwrap(),
            vout: utxo.outpoint.as_ref().unwrap().vout,
        };
        let tracked_inputs = [outpoint].into_iter().collect();

        let result = create_psbt_from_utxos(session, vec![utxo], tracked_inputs);
        assert!(matches!(result.unwrap_err(), SweepError::NoAvailableUtxos));
    }

    #[test]
    fn test_basic_sweep_psbt_creation() {
        let session = create_test_session();
        let utxos =
            vec![create_test_utxo(50_000, 0, None), create_test_utxo(100_000, 1, Some([0xaa; 20]))];

        let psbt = create_psbt_from_utxos(session, utxos, HashSet::new()).unwrap();

        assert_eq!(psbt.inputs.len(), 2);
        assert_eq!(psbt.outputs.len(), 1);
        assert_eq!(psbt.unsigned_tx.version, Version::TWO);
        assert_eq!(psbt.unsigned_tx.lock_time, LockTime::ZERO);

        // Verify FROST compatibility
        for input in &psbt.inputs {
            assert!(input.witness_utxo.is_some());
            assert!(input.sighash_type.is_some());
            assert_eq!(input.sighash_type.unwrap(), PsbtSighashType::from(TapSighashType::Default));
        }
    }

    #[test]
    fn test_utxo_filtering() {
        let session = create_test_session();
        let utxo1 = create_test_utxo(50_000, 0, None);
        let utxo2 = create_test_utxo(75_000, 1, None);

        let outpoint1 = OutPoint {
            txid: Txid::from_slice(&utxo1.outpoint.as_ref().unwrap().txid).unwrap(),
            vout: utxo1.outpoint.as_ref().unwrap().vout,
        };

        let tracked_inputs = [outpoint1].into_iter().collect();
        let psbt = create_psbt_from_utxos(session, vec![utxo1, utxo2], tracked_inputs).unwrap();

        // Should only use utxo2
        assert_eq!(psbt.inputs.len(), 1);
        assert_eq!(psbt.inputs[0].witness_utxo.as_ref().unwrap().value, Amount::from_sat(75_000));
    }

    #[test]
    fn test_insufficient_funds() {
        let session = WalletSweepSession {
            fee_rate_sat_vb: 500, // High but within bounds fee rate
            ..create_test_session()
        };
        let utxo = create_test_utxo(1_000, 0, None); // Small UTXO

        let result = create_psbt_from_utxos(session, vec![utxo], HashSet::new());
        assert!(matches!(result.unwrap_err(), SweepError::InsufficientFunds { .. }));
    }

    // Valuable tests - important features
    #[test]
    fn test_fee_calculation() {
        let inputs = vec![create_test_input(100_000, 0, None)];
        let output = TxOut {
            value: Amount::from_sat(95_000),
            script_pubkey: ScriptBuf::from_bytes(
                vec![0x51, 0x20].into_iter().chain(std::iter::repeat(0x11).take(32)).collect(),
            ),
        };

        let psbt = build_preliminary_psbt(&inputs, &output);
        let fee_rate = FeeRate::from_sat_per_vb(10).unwrap();
        let fee = calculate_transaction_fee(&psbt, fee_rate).unwrap();

        assert!(fee > Amount::ZERO);
        assert!(fee < Amount::from_sat(10_000));
    }

    #[test]
    fn test_psbt_metadata() {
        let session = create_test_session();
        let eth_addr = [0xaa; 20];
        let utxo = create_test_utxo(100_000, 0, Some(eth_addr));

        let psbt = create_psbt_from_utxos(session, vec![utxo], HashSet::new()).unwrap();

        // Test ETH address metadata
        let stored_eth_addr =
            psbt.inputs[0].proprietary.get(&ETH_ADDRESS_KEY.clone()).and_then(|bytes| {
                if bytes.len() == 20 {
                    let mut addr = [0u8; 20];
                    addr.copy_from_slice(bytes);
                    Some(addr)
                } else {
                    None
                }
            });
        assert_eq!(stored_eth_addr, Some(eth_addr));

        // Test version metadata
        assert!(psbt.inputs[0].proprietary.contains_key(&UTXO_VERSION_KEY.clone()));
        let version_bytes = psbt.inputs[0].proprietary.get(&UTXO_VERSION_KEY.clone()).unwrap();
        assert_eq!(version_bytes.len(), 4);
        let version = u32::from_le_bytes(version_bytes.as_slice().try_into().unwrap());
        assert_eq!(version, UtxoVersion::V1 as u32);
    }

    #[test]
    fn test_tracked_transaction_extraction() {
        let outpoint1 = OutPoint::new(
            Txid::from_str("1111111111111111111111111111111111111111111111111111111111111111")
                .unwrap(),
            0,
        );
        let outpoint2 = OutPoint::new(
            Txid::from_str("2222222222222222222222222222222222222222222222222222222222222222")
                .unwrap(),
            1,
        );

        let tracked_tx = create_tracked_transaction(vec![outpoint1, outpoint2]);
        let extracted = extract_tracked_outpoints(&[tracked_tx]);

        assert_eq!(extracted.len(), 2);
        assert!(extracted.contains(&outpoint1));
        assert!(extracted.contains(&outpoint2));
    }

    // Size limiting tests
    #[test]
    fn test_size_limits_with_many_utxos() {
        let session = create_test_session();
        // Create a moderate number of UTXOs to test size limiting
        let utxos: Vec<_> = (0..1500) // Use fewer UTXOs to stay within limits
            .map(|i| create_test_utxo(10_000, i as u32, None))
            .collect();

        let psbt = create_psbt_from_utxos(session, utxos, HashSet::new()).unwrap();

        // Should be limited appropriately
        assert!(psbt.inputs.len() <= MAX_EMERGENCY_SWEEP_INPUTS);
        assert_eq!(psbt.outputs.len(), 1);

        // Should have a reasonable number of inputs
        assert!(psbt.inputs.len() > 500);
        assert!(psbt.inputs.len() <= MAX_EMERGENCY_SWEEP_INPUTS);

        // Verify the transaction meets weight limits
        let weight = calculate_transaction_weight(&psbt).unwrap();
        assert!(
            weight <= EMERGENCY_SWEEP_WEIGHT_LIMIT,
            "Transaction weight {} exceeds limit {}",
            weight,
            EMERGENCY_SWEEP_WEIGHT_LIMIT
        );
    }

    #[test]
    fn test_transaction_weight_calculation() {
        let session = create_test_session();
        let utxos = vec![create_test_utxo(50_000, 0, None), create_test_utxo(75_000, 1, None)];

        let psbt = create_psbt_from_utxos(session, utxos, HashSet::new()).unwrap();
        let weight = calculate_transaction_weight(&psbt).unwrap();

        // Weight should be reasonable for a 2-input transaction
        assert!(weight > 0);
        assert!(weight < EMERGENCY_SWEEP_WEIGHT_LIMIT);

        // Based on actual measurement: 2-input transaction ≈ 626 WU
        // Allow reasonable range around this value
        assert!(weight > 600 && weight < 700);
    }

    #[test]
    fn test_weight_limits_enforcement() {
        let session = create_test_session();
        let utxos = vec![create_test_utxo(100_000, 0, None)];

        let psbt = create_psbt_from_utxos(session, utxos, HashSet::new()).unwrap();

        // Should pass size verification
        verify_transaction_size_limits(&psbt).unwrap();

        let weight = calculate_transaction_weight(&psbt).unwrap();
        assert!(weight <= EMERGENCY_SWEEP_WEIGHT_LIMIT);
    }

    #[test]
    fn test_utxo_sorting_by_value() {
        let session = create_test_session();
        let utxos = vec![
            create_test_utxo(10_000, 0, None),  // smallest
            create_test_utxo(100_000, 1, None), // largest
            create_test_utxo(50_000, 2, None),  // medium
        ];

        let psbt = create_psbt_from_utxos(session, utxos, HashSet::new()).unwrap();

        // Should have all 3 inputs since we're under limits
        assert_eq!(psbt.inputs.len(), 3);

        // All UTXOs should be included regardless of order (sorting is internal)
        let total_value: Amount =
            psbt.inputs.iter().map(|input| input.witness_utxo.as_ref().unwrap().value).sum();
        assert_eq!(total_value, Amount::from_sat(160_000)); // 10k + 100k + 50k
    }

    #[test]
    fn test_size_limit_constants() {
        // Verify our constants are reasonable
        assert!(MAX_EMERGENCY_SWEEP_INPUTS > 0);
        assert!(MAX_EMERGENCY_SWEEP_INPUTS <= 2000); // Reasonable upper bound

        // Emergency sweep uses standard limit for maximum throughput
        assert_eq!(EMERGENCY_SWEEP_WEIGHT_LIMIT, 400_000);
        assert!(EMERGENCY_SWEEP_WEIGHT_LIMIT < 4_000_000); // Bitcoin consensus limit

        // Verify our calculation math
        let estimated_max_inputs = (EMERGENCY_SWEEP_WEIGHT_LIMIT - EMERGENCY_SWEEP_BASE_WEIGHT) /
            TAPROOT_KEYSPEND_INPUT_WEIGHT;
        assert!(MAX_EMERGENCY_SWEEP_INPUTS as u64 <= estimated_max_inputs);
    }

    #[test]
    fn test_apply_size_limits_function() {
        // Test the apply_size_limits function directly
        let utxos: Vec<_> =
            (0..10).map(|i| create_test_utxo((i + 1) * 10_000, i as u32, None)).collect();

        let limited = apply_size_limits(utxos).unwrap();

        // Should return all UTXOs since 10 is well under the limit
        assert_eq!(limited.len(), 10);

        // Test with excessive UTXOs
        let many_utxos: Vec<_> = (0..2200) // Use 2200 to clearly exceed new limits
            .map(|i| create_test_utxo(1000, i as u32, None))
            .collect();

        let limited = apply_size_limits(many_utxos).unwrap();

        // Should be limited to our maximum
        assert!(limited.len() <= MAX_EMERGENCY_SWEEP_INPUTS);
        assert!(limited.len() <= 1800); // Should be at or under our max
        assert!(limited.len() > 1000); // Should still be substantial
    }

    #[test]
    fn test_dust_threshold_validation() {
        let session = create_test_session();

        // Test dust threshold validation with P2WPKH destination address (294 sats dust threshold)
        // Using 5 sat/vB fee rate to calculate the required UTXO value that results in dust output
        let dust_session = WalletSweepSession {
            fee_rate_sat_vb: 5, // Low fee rate
            ..session
        };

        // Use a UTXO value that will result in an output below the dust threshold after fee
        // deduction
        let utxo = create_test_utxo(750, 0, None); // Should result in output < 294 sats (P2WPKH dust threshold)

        let result = create_psbt_from_utxos(dust_session.clone(), vec![utxo], HashSet::new());

        match result {
            Err(SweepError::OutputBelowDustThreshold { value, dust_threshold }) => {
                println!(
                    "Correctly caught dust output: {} sats < {} sats threshold",
                    value, dust_threshold
                );
                assert!(value < dust_threshold);
                assert_eq!(dust_threshold, 294); // P2WPKH dust threshold
            }
            Err(SweepError::InsufficientFunds { fee, total_value }) => {
                // This might happen if fee is higher than total value
                println!(
                    "Got insufficient funds: fee={} sats, total={} sats",
                    fee.to_sat(),
                    total_value.to_sat()
                );
                assert!(fee.to_sat() >= total_value.to_sat());

                // If we get insufficient funds, let's try with a slightly larger UTXO
                // that should pass insufficient funds check but fail dust check
                let slightly_larger_utxo = create_test_utxo(780, 0, None);
                let result2 = create_psbt_from_utxos(
                    dust_session.clone(),
                    vec![slightly_larger_utxo],
                    HashSet::new(),
                );
                match result2 {
                    Err(SweepError::OutputBelowDustThreshold { .. }) => {
                        // This is what we wanted to test
                    }
                    other => {
                        panic!("Expected dust threshold error with 780 sat UTXO, got: {:?}", other)
                    }
                }
            }
            Err(other_error) => {
                panic!(
                    "Expected OutputBelowDustThreshold or InsufficientFunds but got: {:?}",
                    other_error
                );
            }
            Ok(psbt) => {
                let output_value = psbt.unsigned_tx.output[0].value.to_sat();
                let expected_dust_threshold = 294; // P2WPKH dust threshold
                if output_value < expected_dust_threshold {
                    panic!(
                        "Output {} sats is below dust threshold {} but wasn't caught!",
                        output_value, expected_dust_threshold
                    );
                } else {
                    panic!(
                        "Expected an error but got success with output value: {} sats",
                        output_value
                    );
                }
            }
        }
    }

    #[test]
    fn test_value_overflow_validation() {
        let session = create_test_session();

        // Create UTXO with value exceeding maximum Bitcoin value
        let mut utxo = create_test_utxo(1000, 0, None);
        utxo.output.as_mut().unwrap().value = MAX_BITCOIN_VALUE_SATS + 1;

        let result = create_psbt_from_utxos(session, vec![utxo], HashSet::new());

        // Should fail due to invalid Bitcoin amount
        assert!(matches!(result.unwrap_err(), SweepError::InvalidBitcoinAmount { .. }));
    }

    #[test]
    fn test_calculate_total_value_overflow_protection() {
        // Test individual UTXO value exceeding maximum Bitcoin value
        let mut utxo_too_large = create_test_utxo(1000, 0, None);
        utxo_too_large.output.as_mut().unwrap().value = MAX_BITCOIN_VALUE_SATS + 1;

        let result = calculate_total_value(&[utxo_too_large]);
        assert!(matches!(result.unwrap_err(), SweepError::InvalidBitcoinAmount { .. }));

        // Test sum exceeding maximum Bitcoin value (even if individual UTXOs are valid)
        let large_value = MAX_BITCOIN_VALUE_SATS / 2 + 1;
        let utxo1 = create_test_utxo(large_value, 0, None);
        let utxo2 = create_test_utxo(large_value, 1, None);

        let result = calculate_total_value(&[utxo1, utxo2]);
        assert!(matches!(result.unwrap_err(), SweepError::InvalidBitcoinAmount { .. }));

        // Test valid sum that doesn't overflow
        let valid_utxos = vec![
            create_test_utxo(100_000, 0, None),
            create_test_utxo(200_000, 1, None),
            create_test_utxo(300_000, 2, None),
        ];

        let result = calculate_total_value(&valid_utxos).unwrap();
        assert_eq!(result, Amount::from_sat(600_000));

        // Test empty UTXO list
        let empty_result = calculate_total_value(&[]).unwrap();
        assert_eq!(empty_result, Amount::ZERO);
    }

    #[test]
    fn test_rbf_sequence_enabled() {
        let session = create_test_session();
        let utxos = vec![create_test_utxo(100_000, 0, None)];

        let psbt = create_psbt_from_utxos(session, utxos, HashSet::new()).unwrap();

        // Verify RBF is enabled (sequence < 0xfffffffe)
        for input in &psbt.unsigned_tx.input {
            assert_eq!(input.sequence, bitcoin::Sequence::ENABLE_RBF_NO_LOCKTIME);
            assert!(input.sequence.is_rbf());
        }
    }
    //
    // #[test]
    // fn test_error_specificity() {
    //     let session = WalletSweepSession {
    //         coordinator_signature: vec![], // Empty signature should cause InvalidRequest
    //         ..create_test_session()
    //     };
    //
    //     let utxos = vec![create_test_utxo(100_000, 0, None)];
    //     let result = create_psbt_from_utxos(session, utxos, HashSet::new());
    //
    //     // Should fail with InvalidRequest, not DataParsing
    //     assert!(matches!(result.unwrap_err(), SweepError::InvalidRequest(_)));
    // }

    // #[test]
    // fn test_coordinator_signature_validation() {
    //     // Test empty signature
    //     let session_empty = WalletSweepSession { ..create_test_session() };
    //     let utxos = vec![create_test_utxo(100_000, 0, None)];
    //     let result = create_psbt_from_utxos(session_empty, utxos.clone(), HashSet::new());
    //     assert!(matches!(result.unwrap_err(), SweepError::InvalidRequest(_)));
    //
    //     // Test short signature
    //     let session_short = WalletSweepSession {
    //         coordinator_signature: vec![0x01, 0x02, 0x03], // Only 3 bytes
    //         ..create_test_session()
    //     };
    //     let result = create_psbt_from_utxos(session_short, utxos, HashSet::new());
    //     assert!(matches!(result.unwrap_err(), SweepError::InvalidRequest(_)));
    // }

    #[test]
    fn test_network_extraction_from_address() {
        use bitcoin::Address;
        use std::str::FromStr;

        // Test mainnet address
        let mainnet_addr = Address::from_str("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
            .unwrap()
            .as_unchecked()
            .clone();
        assert_eq!(extract_network_from_address(&mainnet_addr).unwrap(), bitcoin::Network::Bitcoin);

        // Test testnet address
        let testnet_addr = Address::from_str("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx")
            .unwrap()
            .as_unchecked()
            .clone();
        assert_eq!(extract_network_from_address(&testnet_addr).unwrap(), bitcoin::Network::Testnet);

        // Test regtest address
        let regtest_addr = Address::from_str("bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080")
            .unwrap()
            .as_unchecked()
            .clone();
        assert_eq!(extract_network_from_address(&regtest_addr).unwrap(), bitcoin::Network::Regtest);
    }

    #[test]
    fn test_network_validation() {
        let session = create_test_session();
        let utxos = vec![create_test_utxo(100_000, 0, None)];

        // The test address in create_test_session() is a mainnet address,
        // so network extraction should work correctly
        let result = create_psbt_from_utxos(session, utxos, HashSet::new());

        // Should either succeed or fail on other validation, but not on network extraction
        if let Err(e) = result {
            assert!(
                !matches!(e, SweepError::AddressValidation(_)),
                "Network extraction should work for valid mainnet address"
            );
        }
    }

    #[test]
    fn test_invalid_utxo_scenarios() {
        let session = create_test_session();

        // Test UTXO with invalid script length (non-P2TR)
        let mut utxo = create_test_utxo(100_000, 0, None);
        // Create a non-P2TR script (P2PKH = 25 bytes)
        utxo.output.as_mut().unwrap().script_pubkey.as_mut().unwrap().script = vec![0; 25];

        let result = create_psbt_from_utxos(session.clone(), vec![utxo], HashSet::new());
        assert!(matches!(result.unwrap_err(), SweepError::InvalidUtxo(_)));

        // Test UTXO with zero value
        let mut utxo_zero = create_test_utxo(100_000, 0, None);
        utxo_zero.output.as_mut().unwrap().value = 0;

        let result = create_psbt_from_utxos(session.clone(), vec![utxo_zero], HashSet::new());
        assert!(matches!(result.unwrap_err(), SweepError::InvalidUtxo(_)));

        // Test UTXO missing outpoint
        let mut utxo_no_outpoint = create_test_utxo(100_000, 0, None);
        utxo_no_outpoint.outpoint = None;

        let result =
            create_psbt_from_utxos(session.clone(), vec![utxo_no_outpoint], HashSet::new());
        assert!(matches!(result.unwrap_err(), SweepError::InvalidUtxo(_)));
    }

    #[test]
    fn test_fee_rate_edge_cases() {
        let utxos = vec![create_test_utxo(100_000, 0, None)];

        // Test zero fee rate
        let session_zero_fee = WalletSweepSession { fee_rate_sat_vb: 0, ..create_test_session() };
        let result = create_psbt_from_utxos(session_zero_fee, utxos.clone(), HashSet::new());
        assert!(matches!(result.unwrap_err(), SweepError::InvalidFeeRate { .. }));

        // Test fee rate below minimum
        let session_low_fee = WalletSweepSession {
            fee_rate_sat_vb: MIN_FEE_RATE_SAT_VB - 1,
            ..create_test_session()
        };
        let result = create_psbt_from_utxos(session_low_fee, utxos.clone(), HashSet::new());
        assert!(matches!(result.unwrap_err(), SweepError::InvalidFeeRate { .. }));

        // Test fee rate above maximum
        let session_high_fee = WalletSweepSession {
            fee_rate_sat_vb: MAX_FEE_RATE_SAT_VB + 1,
            ..create_test_session()
        };
        let result = create_psbt_from_utxos(session_high_fee, utxos, HashSet::new());
        assert!(matches!(result.unwrap_err(), SweepError::InvalidFeeRate { .. }));
    }

    #[test]
    fn test_transaction_too_large_scenario() {
        // This test is challenging because we need to create a scenario where
        // the PSBT passes initial size limits but fails final verification
        let session = create_test_session();

        // Create exactly at the limit to test boundary conditions
        let max_utxos = MAX_EMERGENCY_SWEEP_INPUTS;
        let utxos: Vec<_> =
            (0..max_utxos).map(|i| create_test_utxo(10_000, i as u32, None)).collect();

        let psbt = create_psbt_from_utxos(session, utxos, HashSet::new()).unwrap();

        // Verify we're close to but under the limit
        let weight = calculate_transaction_weight(&psbt).unwrap();
        assert!(weight <= EMERGENCY_SWEEP_WEIGHT_LIMIT);
        assert!(weight > EMERGENCY_SWEEP_WEIGHT_LIMIT / 2); // Should be substantial

        // The transaction should be valid (this mainly tests our constants are correct)
        verify_transaction_size_limits(&psbt).unwrap();
    }

    #[test]
    fn test_psbt_serialization_preserves_ethereum_addresses() {
        let session = create_test_session();

        // Create UTXOs with different Ethereum addresses
        // Note: UTXOs are sorted by value (largest first), so 200k will be first
        let eth_addr1 = [0xaa; 20]; // Will be on 100k UTXO (index 2 after sorting)
        let eth_addr2 = [0xbb; 20]; // Will be on 200k UTXO (index 0 after sorting)
        let utxos = vec![
            create_test_utxo(100_000, 0, Some(eth_addr1)), // Sorted position: 2
            create_test_utxo(200_000, 1, Some(eth_addr2)), // Sorted position: 0 (largest)
            create_test_utxo(150_000, 2, None),            // Sorted position: 1 (middle)
        ];

        // Create original PSBT
        let original_psbt = create_psbt_from_utxos(session, utxos, HashSet::new()).unwrap();

        // Verify original PSBT has the expected metadata
        assert_eq!(original_psbt.inputs.len(), 3);

        // After sorting by value (largest first):
        // Index 0: 200k UTXO with eth_addr2
        // Index 1: 150k UTXO with no ETH address
        // Index 2: 100k UTXO with eth_addr1

        // Check first input (200k UTXO) has eth_addr2
        let eth2_stored =
            original_psbt.inputs[0].proprietary.get(&ETH_ADDRESS_KEY.clone()).and_then(|bytes| {
                if bytes.len() == 20 {
                    let mut addr = [0u8; 20];
                    addr.copy_from_slice(bytes);
                    Some(addr)
                } else {
                    None
                }
            });
        assert_eq!(eth2_stored, Some(eth_addr2));

        // Check second input (150k UTXO) has no ETH address
        assert!(!original_psbt.inputs[1].proprietary.contains_key(&ETH_ADDRESS_KEY.clone()));

        // Check third input (100k UTXO) has eth_addr1
        let eth1_stored =
            original_psbt.inputs[2].proprietary.get(&ETH_ADDRESS_KEY.clone()).and_then(|bytes| {
                if bytes.len() == 20 {
                    let mut addr = [0u8; 20];
                    addr.copy_from_slice(bytes);
                    Some(addr)
                } else {
                    None
                }
            });
        assert_eq!(eth1_stored, Some(eth_addr1));

        // Verify all inputs have UTXO version metadata
        for input in &original_psbt.inputs {
            assert!(input.proprietary.contains_key(&UTXO_VERSION_KEY.clone()));
            let version_bytes = input.proprietary.get(&UTXO_VERSION_KEY.clone()).unwrap();
            assert_eq!(version_bytes.len(), 4);
            let version = u32::from_le_bytes(version_bytes.as_slice().try_into().unwrap());
            assert_eq!(version, UtxoVersion::V1 as u32);
        }

        // Serialize the PSBT (this is what gets distributed to other nodes)
        let serialized_bytes = original_psbt.serialize();

        // Verify serialization includes all data (should be reasonable size)
        println!("PSBT serialization size: {} bytes", serialized_bytes.len());
        assert!(serialized_bytes.len() > 300); // Should be reasonably large with all metadata

        // Deserialize the PSBT (simulating what other nodes would do)
        let deserialized_psbt = bitcoin::Psbt::deserialize(&serialized_bytes)
            .expect("PSBT should deserialize successfully");

        // Verify deserialized PSBT has identical structure
        assert_eq!(deserialized_psbt.inputs.len(), original_psbt.inputs.len());
        assert_eq!(deserialized_psbt.outputs.len(), original_psbt.outputs.len());
        assert_eq!(
            deserialized_psbt.unsigned_tx.input.len(),
            original_psbt.unsigned_tx.input.len()
        );
        assert_eq!(
            deserialized_psbt.unsigned_tx.output.len(),
            original_psbt.unsigned_tx.output.len()
        );

        // Verify all Ethereum addresses are preserved (accounting for sorted order)
        // Index 0: 200k UTXO with eth_addr2
        let deserialized_eth2 = deserialized_psbt.inputs[0]
            .proprietary
            .get(&ETH_ADDRESS_KEY.clone())
            .and_then(|bytes| {
                if bytes.len() == 20 {
                    let mut addr = [0u8; 20];
                    addr.copy_from_slice(bytes);
                    Some(addr)
                } else {
                    None
                }
            });
        assert_eq!(
            deserialized_eth2,
            Some(eth_addr2),
            "First input (200k UTXO) should have eth_addr2"
        );

        // Index 1: 150k UTXO with no ETH address
        assert!(
            !deserialized_psbt.inputs[1].proprietary.contains_key(&ETH_ADDRESS_KEY.clone()),
            "Second input (150k UTXO) should not have ETH address"
        );

        // Index 2: 100k UTXO with eth_addr1
        let deserialized_eth1 = deserialized_psbt.inputs[2]
            .proprietary
            .get(&ETH_ADDRESS_KEY.clone())
            .and_then(|bytes| {
                if bytes.len() == 20 {
                    let mut addr = [0u8; 20];
                    addr.copy_from_slice(bytes);
                    Some(addr)
                } else {
                    None
                }
            });
        assert_eq!(
            deserialized_eth1,
            Some(eth_addr1),
            "Third input (100k UTXO) should have eth_addr1"
        );

        // Verify all UTXO version metadata is preserved
        for (i, input) in deserialized_psbt.inputs.iter().enumerate() {
            assert!(
                input.proprietary.contains_key(&UTXO_VERSION_KEY.clone()),
                "Input {} should have UTXO version metadata",
                i
            );
            let version_bytes = input.proprietary.get(&UTXO_VERSION_KEY.clone()).unwrap();
            assert_eq!(version_bytes.len(), 4);
            let version = u32::from_le_bytes(version_bytes.as_slice().try_into().unwrap());
            assert_eq!(version, UtxoVersion::V1 as u32, "Input {} should have correct version", i);
        }

        // Verify witness UTXOs are preserved (required for FROST signing)
        for (i, input) in deserialized_psbt.inputs.iter().enumerate() {
            assert!(input.witness_utxo.is_some(), "Input {} should have witness UTXO", i);
            assert!(input.sighash_type.is_some(), "Input {} should have sighash type", i);
            assert_eq!(
                input.sighash_type.unwrap(),
                PsbtSighashType::from(TapSighashType::Default),
                "Input {} should have correct sighash type",
                i
            );
        }

        // Verify the PSBTs are functionally identical (byte-for-byte comparison)
        let original_serialized = original_psbt.serialize();
        let deserialized_serialized = deserialized_psbt.serialize();
        assert_eq!(
            original_serialized, deserialized_serialized,
            "Serialized PSBTs should be identical after round-trip"
        );
    }

    #[test]
    fn test_psbt_base64_and_hex_encoding_compatibility() {
        let session = create_test_session();
        let eth_addr = [
            0xde, 0xad, 0xbe, 0xef, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99,
            0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        ];
        let utxos = vec![create_test_utxo(100_000, 0, Some(eth_addr))];

        let psbt = create_psbt_from_utxos(session, utxos, HashSet::new()).unwrap();
        let psbt_bytes = psbt.serialize();

        // Test base64 encoding (current implementation)
        use bitcoin::base64::{engine::general_purpose, Engine as _};
        let psbt_base64 = general_purpose::STANDARD.encode(&psbt_bytes);

        // Test hex encoding (suggested for tool compatibility)
        let psbt_hex = hex::encode(&psbt_bytes);

        // Verify both encodings can be decoded back to the same bytes
        let decoded_from_base64 =
            general_purpose::STANDARD.decode(&psbt_base64).expect("Base64 decoding should work");
        let decoded_from_hex = hex::decode(&psbt_hex).expect("Hex decoding should work");

        assert_eq!(psbt_bytes, decoded_from_base64, "Base64 round-trip should preserve data");
        assert_eq!(psbt_bytes, decoded_from_hex, "Hex round-trip should preserve data");
        assert_eq!(
            decoded_from_base64, decoded_from_hex,
            "Both encodings should produce same result"
        );

        // Verify the decoded PSBTs can be deserialized and have the ETH address
        let psbt_from_base64 = bitcoin::Psbt::deserialize(&decoded_from_base64).unwrap();
        let psbt_from_hex = bitcoin::Psbt::deserialize(&decoded_from_hex).unwrap();

        // Check ETH address is preserved in both
        for psbt_variant in [&psbt_from_base64, &psbt_from_hex] {
            let stored_eth_addr = psbt_variant.inputs[0]
                .proprietary
                .get(&ETH_ADDRESS_KEY.clone())
                .and_then(|bytes| {
                    if bytes.len() == 20 {
                        let mut addr = [0u8; 20];
                        addr.copy_from_slice(bytes);
                        Some(addr)
                    } else {
                        None
                    }
                });
            assert_eq!(stored_eth_addr, Some(eth_addr));
        }
    }
}
