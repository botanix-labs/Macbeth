use super::error::Error;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Default, Clone, Copy, PartialEq)]
pub enum UtxoVersion {
    /// Original version with taproot key spend path
    #[default]
    V1 = 0,
}

impl TryFrom<u32> for UtxoVersion {
    type Error = Error;

    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(UtxoVersion::V1),
            _ => Err(Error::InvalidUTXOVersion(v)),
        }
    }
}
