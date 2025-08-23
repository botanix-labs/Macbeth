use bdk_wallet::psbt::PsbtUtils;
use bitcoin::{secp256k1, FeeRate, Psbt, Weight};
use frost_secp256k1_tr as frost;
use thiserror::Error;

use crate::wallet::{
    MAX_BASE_TX_WEIGHT, MAX_TX_WEIGHT, PER_OUTPUT_MAX_WEIGHT, PER_P2TR_KEYSPEND_WEIGHT,
    SEGWIT_FLAG_WEIGHT, SEGWIT_MARKER_WEIGHT, TAPROOT_KEYSPEND_SIGHASH_DEFAULT_WEIGHT,
};

#[derive(Debug, Error)]
pub enum ParsingError {
    #[error("invalid frost id")]
    InvalidFrostPeerId,
    #[error("invalid signing session id")]
    InvalidSigningSessionId,
    #[error("invalid eth address: {0}")]
    InvalidEthAddress(&'static str),
}

#[derive(Debug, Clone, Error)]
pub enum VerifyingKeyExtError {
    #[error("Failed to convert to secp pk: {0}")]
    FailedToConvertToSecpPk(bitcoin::secp256k1::Error),
    #[error("Frost error: {0}")]
    FrostError(#[from] frost::Error),
    #[error("Failed to convert to secp pk: {0}")]
    FailedToConvertToFrostPk(frost::Error),
}

#[derive(Debug, Error)]
pub enum WalletCalculationError {
    #[error("Transaction weight overflow")]
    WeightOverflow,
    #[error("Invalid parameters")]
    InvalidParameters,
}

/// Extension trait for Frost verifying key (aggregate key)
pub trait VerifyingKeyExt: Into<frost::VerifyingKey> {
    fn to_secp_pk(self) -> Result<bitcoin::secp256k1::PublicKey, VerifyingKeyExtError> {
        let vk: frost::VerifyingKey = self.into();
        let pk = bitcoin::secp256k1::PublicKey::from_slice(vk.serialize()?.as_slice())
            .map_err(VerifyingKeyExtError::FailedToConvertToSecpPk)?;

        Ok(pk)
    }

    #[allow(unused)]
    fn from_secp_pk(
        pk: &secp256k1::PublicKey,
    ) -> Result<frost::VerifyingKey, VerifyingKeyExtError> {
        let vk = frost::VerifyingKey::deserialize(pk.serialize().as_slice())
            .map_err(VerifyingKeyExtError::FailedToConvertToFrostPk)?;

        Ok(vk)
    }
}

impl VerifyingKeyExt for frost::VerifyingKey {}

/// Calculates the total weight of a PSBT after it has been fully signed with P2TR keyspend inputs.
pub fn calculate_signed_tx_weight(psbt: &Psbt) -> Result<Weight, WalletCalculationError> {
    let unsigned_tx_weight = psbt.unsigned_tx.weight();

    // calculate the weight of the signatures (assuming all inputs are p2tr)
    let num_inputs = psbt.inputs.len();
    let per_input_witness_item_count = Weight::from_wu(1);
    let total_signature_weight = (TAPROOT_KEYSPEND_SIGHASH_DEFAULT_WEIGHT +
        per_input_witness_item_count)
        .checked_mul(num_inputs as u64)
        .ok_or(WalletCalculationError::WeightOverflow)?; // or changeoverflow

    // total including base weights for segwit transactions. use checked add to avoid overflow
    unsigned_tx_weight
        .checked_add(total_signature_weight)
        .and_then(|w| w.checked_add(SEGWIT_FLAG_WEIGHT))
        .and_then(|w| w.checked_add(SEGWIT_MARKER_WEIGHT))
        .ok_or(WalletCalculationError::WeightOverflow)
}

// Calculates the fee rate of a PSBT after it has been fully signed with P2TR keyspend inputs.
pub fn calculate_signed_tx_fee_rate(psbt: &Psbt) -> Result<FeeRate, WalletCalculationError> {
    let tx_weight = calculate_signed_tx_weight(psbt)?;
    if tx_weight.to_wu() == 0 {
        return Err(WalletCalculationError::InvalidParameters);
    }

    let absolute_fee = psbt.fee_amount().unwrap();
    let fee_rate = FeeRate::from_sat_per_kwu((absolute_fee.to_sat() * 1000) / tx_weight.to_wu());
    Ok(fee_rate)
}

/// Returns the maximum number of inputs that can be added to a PSBT without exceeding the maximum
/// transaction weight.
pub fn max_number_of_psbt_inputs(num_pegouts: u64) -> u64 {
    let max_outputs = num_pegouts + 1; // +1 for change output
    let psbt_without_inputs_weight = MAX_BASE_TX_WEIGHT + max_outputs * PER_OUTPUT_MAX_WEIGHT;
    let max_number_of_inputs =
        (MAX_TX_WEIGHT - psbt_without_inputs_weight) / PER_P2TR_KEYSPEND_WEIGHT;
    max_number_of_inputs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        database::version::UtxoVersion,
        test_utils::{
            add_dummy_signatures_to_psbt, create_random_pegout_id, random_compute_txid,
            random_p2tr_keyspend_script,
        },
        util::UPPER_PEGOUT_BOUND,
        wallet::{
            psbt::{create_psbt, InputDTO},
            MAX_TX_WEIGHT,
        },
    };

    use bitcoin::{Amount, OutPoint, TapSighashType, TxOut};

    #[test]
    fn test_calculate_signed_tx_weight_and_fee_rate() {
        let test_cases = vec![
            ("1_input_1_output", psbt_1_input_1_output()),
            ("2_inputs_2_outputs", psbt_2_inputs_2_outputs()),
            ("4_inputs_2_outputs", psbt_4_inputs_2_outputs()),
        ];

        for (name, psbt) in test_cases {
            let mut psbt_with_signatures = psbt;
            add_dummy_signatures_to_psbt(&mut psbt_with_signatures, TapSighashType::Default);
            let tx = psbt_with_signatures.clone().extract_tx().expect("Failed to extract tx");

            let expected_fee_rate = psbt_with_signatures.fee_rate().unwrap();
            let expected_weight = tx.weight();

            let calculated_weight =
                calculate_signed_tx_weight(&psbt_with_signatures).expect("should not fail");
            let calculated_fee_rate =
                calculate_signed_tx_fee_rate(&psbt_with_signatures).expect("should not fail");

            assert_eq!(calculated_weight.to_wu(), expected_weight.to_wu(), "{}", name);
            assert_eq!(
                calculated_fee_rate.to_sat_per_kwu(),
                expected_fee_rate.to_sat_per_kwu(),
                "{}",
                name
            );
        }
    }

    fn create_random_input(value_sats: u64) -> InputDTO {
        InputDTO {
            outpoint: OutPoint::new(random_compute_txid(), 0),
            output: TxOut {
                value: Amount::from_sat(value_sats),
                script_pubkey: random_p2tr_keyspend_script(),
            },
            eth_address: None,
            version: UtxoVersion::default(),
        }
    }

    // Example based on mainnet pegout tx https://mempool.space/tx/fc9aaae314c366956bf74e184ba3759b56031ab7c9eddf206d3231ca382abbe8
    fn psbt_2_inputs_2_outputs() -> Psbt {
        let mut inputs = vec![];
        let input1 = create_random_input(3_000);
        inputs.push(input1);
        let input2 = create_random_input(2_384_042);
        inputs.push(input2);

        let mut outputs = vec![];
        let output1 = (
            TxOut {
                value: Amount::from_sat(2_362_047),
                script_pubkey: random_p2tr_keyspend_script(), // Recipient script
            },
            create_random_pegout_id().as_bytes(),
        );
        outputs.push(output1);

        let change = Some(TxOut {
            value: Amount::from_sat(23_282),
            script_pubkey: random_p2tr_keyspend_script(), // Change script
        });

        create_psbt(inputs, outputs, change)
    }

    // Example based on mainnet pegout tx https://mempool.space/tx/a8a7197d99fedc6b366671a22d9312f7e5ed9869f53e61359995ee96ee65fed8
    fn psbt_4_inputs_2_outputs() -> Psbt {
        let mut inputs = vec![];
        let input1 = create_random_input(9_425);
        inputs.push(input1);
        let input2 = create_random_input(10_000);
        inputs.push(input2);
        let input3 = create_random_input(5_000);
        inputs.push(input3);
        let input4 = create_random_input(498_139);
        inputs.push(input4);

        let mut outputs = vec![];
        let output1 = (
            TxOut {
                value: Amount::from_sat(72_827),
                script_pubkey: random_p2tr_keyspend_script(), // Recipient script
            },
            create_random_pegout_id().as_bytes(),
        );
        outputs.push(output1);

        let change = Some(TxOut {
            value: Amount::from_sat(447_575),
            script_pubkey: random_p2tr_keyspend_script(), // Change script
        });

        create_psbt(inputs, outputs, change)
    }

    fn psbt_1_input_1_output() -> Psbt {
        let mut inputs = vec![];
        let input1 = create_random_input(100_000);
        inputs.push(input1);

        let fee = 445;
        let mut outputs = vec![];
        let output1 = (
            TxOut {
                value: Amount::from_sat(100_000 - fee),
                script_pubkey: random_p2tr_keyspend_script(),
            },
            create_random_pegout_id().as_bytes(),
        );
        outputs.push(output1);

        create_psbt(inputs, outputs, None)
    }

    fn create_signed_tx(num_inputs: u64, num_outputs: u64) -> Psbt {
        let mut inputs = vec![];
        for _ in 0..num_inputs {
            let input = create_random_input(10_000);
            inputs.push(input);
        }

        let mut outputs = vec![];
        for _ in 0..num_outputs {
            let output = (
                TxOut {
                    value: Amount::from_sat(10_000),
                    script_pubkey: random_p2tr_keyspend_script(), // Recipient script
                },
                create_random_pegout_id().as_bytes(),
            );
            outputs.push(output);
        }

        let mut psbt = create_psbt(inputs, outputs, None);
        add_dummy_signatures_to_psbt(&mut psbt, TapSighashType::Default);
        psbt // with signatures
    }

    #[test]
    fn test_max_number_of_psbt_inputs() {
        let max_number_of_inputs = max_number_of_psbt_inputs(UPPER_PEGOUT_BOUND as u64);
        println!("max number of pegout inputs: {}", max_number_of_inputs); // 1364

        // Test 1: Create a pegout with exactly the max number of inputs
        let tx = create_signed_tx(max_number_of_inputs, UPPER_PEGOUT_BOUND as u64);
        let tx_weight = tx.clone().extract_tx().expect("Failed to extract tx").weight();

        // The tx weight should be just below the max transaction weight
        assert!(tx_weight.to_wu() <= MAX_TX_WEIGHT, "tx weight: {}", tx_weight.to_wu());

        // Test 2: Create a pegout with one more input than the max number of inputs
        let tx = create_signed_tx(max_number_of_inputs + 1, UPPER_PEGOUT_BOUND as u64);

        let tx_weight = tx.clone().extract_tx().expect("Failed to extract tx").weight();

        // The tx weight should be above the max transaction weight
        assert!(tx_weight.to_wu() > MAX_TX_WEIGHT, "tx weight: {}", tx_weight.to_wu());
    }

    #[test]
    fn test_max_number_of_psbt_inputs_for_sweep() {
        let max_number_of_inputs = max_number_of_psbt_inputs(0);
        println!("max number of sweep inputs: {}", max_number_of_inputs); // 1738

        // Test 1: Create a sweep with max number of inputs
        let tx = create_signed_tx(max_number_of_inputs, 1);
        let tx_weight = tx.clone().extract_tx().expect("Failed to extract tx").weight();

        // The tx weight should be just below the max transaction weight
        assert!(tx_weight.to_wu() <= MAX_TX_WEIGHT, "tx weight: {}", tx_weight.to_wu());

        // Test 2: Create a sweep with one more input than the max number of inputs
        let tx = create_signed_tx(max_number_of_inputs + 1, 1);
        let tx_weight = tx.clone().extract_tx().expect("Failed to extract tx").weight();

        // The tx weight should be above the max transaction weight
        assert!(tx_weight.to_wu() > MAX_TX_WEIGHT, "tx weight: {}", tx_weight.to_wu());
    }
}
