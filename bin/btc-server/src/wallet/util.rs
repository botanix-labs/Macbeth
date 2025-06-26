use bdk_wallet::psbt::PsbtUtils;
use bitcoin::{secp256k1, FeeRate, Psbt, Weight};
use frost_secp256k1_tr as frost;
use thiserror::Error;

use crate::wallet::{
    SEGWIT_FLAG_WEIGHT, SEGWIT_MARKER_WEIGHT, TAPROOT_KEYSPEND_SATISFACTION_WEIGHT,
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
pub fn calculate_signed_tx_weight(psbt: &Psbt) -> Weight {
    let unsigned_tx_weight = psbt.unsigned_tx.weight();

    // calculate the weight of the signatures (assuming all inputs are p2tr)
    let num_inputs = psbt.inputs.len();
    let per_input_witness_item_count = Weight::from_wu(1);
    let total_signature_weight = (TAPROOT_KEYSPEND_SATISFACTION_WEIGHT
        + per_input_witness_item_count)
        .checked_mul(num_inputs as u64)
        .expect("Bitcoin amounts should never overflow u64");

    // total including base weights for segwit transactions
    unsigned_tx_weight + total_signature_weight + SEGWIT_FLAG_WEIGHT + SEGWIT_MARKER_WEIGHT
}

// Calculates the fee rate of a PSBT after it has been fully signed with P2TR keyspend inputs.
pub fn calculate_signed_tx_fee_rate(psbt: &Psbt) -> FeeRate {
    let tx_weight = calculate_signed_tx_weight(psbt);
    let absolute_fee = psbt.fee_amount().unwrap();
    FeeRate::from_sat_per_kwu((absolute_fee.to_sat() * 1000) / tx_weight.to_wu())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallet::psbt::create_psbt;
    use crate::{
        database::version::UtxoVersion,
        test_utils::{create_random_pegout_id, random_compute_txid, random_p2tr_keyspend_script},
        wallet::psbt::InputDTO,
    };
    use bitcoin::{Amount, OutPoint, TxOut};

    #[test]
    // Test based on mainnet pegout tx https://mempool.space/tx/fc9aaae314c366956bf74e184ba3759b56031ab7c9eddf206d3231ca382abbe8
    fn test_mainnet_example_2_inputs_2_outputs() {
        let psbt = create_mainnet_example_psbt_2_inputs_2_outputs();

        let weight = calculate_signed_tx_weight(&psbt);
        assert_eq!(weight.to_wu(), 848);

        let absolute_fee = psbt.fee_amount().unwrap();
        assert_eq!(absolute_fee.to_sat(), 1_713);

        let fee_rate = calculate_signed_tx_fee_rate(&psbt);
        assert_eq!(fee_rate.to_sat_per_kwu(), 2_020);
    }

    #[test]
    // Test based on mainnet pegout tx https://mempool.space/tx/a8a7197d99fedc6b366671a22d9312f7e5ed9869f53e61359995ee96ee65fed8
    fn test_mainnet_example_4_inputs_2_outputs() {
        let psbt = create_mainnet_example_psbt_4_inputs_2_outputs();

        let weight = calculate_signed_tx_weight(&psbt);
        assert_eq!(weight.to_wu(), 1310);

        let absolute_fee = psbt.fee_amount().unwrap();
        assert_eq!(absolute_fee.to_sat(), 2_162);

        let fee_rate = calculate_signed_tx_fee_rate(&psbt);
        assert_eq!(fee_rate.to_sat_per_kwu(), 1_650);
    }

    #[test]
    // Made up example without change output
    fn test_1_input_1_output() {
        let psbt = create_psbt_1_input_1_output();
        let weight = calculate_signed_tx_weight(&psbt);
        assert_eq!(weight.to_wu(), 445);

        let absolute_fee = psbt.fee_amount().unwrap();
        assert_eq!(absolute_fee.to_sat(), 445);

        let fee_rate = calculate_signed_tx_fee_rate(&psbt);
        assert_eq!(fee_rate.to_sat_per_kwu(), 1000);
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

    fn create_mainnet_example_psbt_2_inputs_2_outputs() -> Psbt {
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

    fn create_mainnet_example_psbt_4_inputs_2_outputs() -> Psbt {
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

    fn create_psbt_1_input_1_output() -> Psbt {
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
}
