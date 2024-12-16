use frost_secp256k1_tr as frost;
use thiserror::Error;

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
}

/// Extension trait for Frost verifying key (aggregate key)
pub trait VerifyingKeyExt: Into<frost::VerifyingKey> {
    fn to_secp_pk(self) -> Result<bitcoin::secp256k1::PublicKey, VerifyingKeyExtError> {
        let vk: frost::VerifyingKey = self.into();
        let pk = bitcoin::secp256k1::PublicKey::from_slice(vk.serialize()?.as_slice())
            .map_err(VerifyingKeyExtError::FailedToConvertToSecpPk)?;

        Ok(pk)
    }
}

impl VerifyingKeyExt for frost::VerifyingKey {}
