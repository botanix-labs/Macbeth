use std::fmt;

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
    FailedToConvertToSecpPk(bitcoin::secp256k1::Error),
}

impl fmt::Display for VerifyingKeyExtError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            VerifyingKeyExtError::FailedToConvertToSecpPk(err) => {
                write!(f, "Failed to convert to secp pk: {}", err)
            }
        }
    }
}
/// Extension trait for Frost verifying key (aggregate key)
pub trait VerifyingKeyExt: Into<frost::VerifyingKey> {
    fn to_secp_pk(self) -> Result<bitcoin::secp256k1::PublicKey, VerifyingKeyExtError> {
        let vk: frost::VerifyingKey = self.into();
        let pk = bitcoin::secp256k1::PublicKey::from_slice(vk.serialize().as_slice())
            .map_err(VerifyingKeyExtError::FailedToConvertToSecpPk)?;

        Ok(pk)
    }
}

impl VerifyingKeyExt for frost::VerifyingKey {}
