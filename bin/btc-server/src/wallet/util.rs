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
