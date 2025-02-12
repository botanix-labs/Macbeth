use super::error::Error;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Default, Clone, Copy, PartialEq)]
pub enum UtxoVersion {
    /// Original version with basic spending conditions
    #[default]
    V1 = 1,
}

impl TryFrom<u32> for UtxoVersion {
    type Error = Error;

    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            1 => Ok(UtxoVersion::V1),
            _ => Err(Error::InvalidUTXOVersion(v)),
        }
    }
}
