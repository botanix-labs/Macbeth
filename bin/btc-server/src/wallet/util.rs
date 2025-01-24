use bitcoin::secp256k1;
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
