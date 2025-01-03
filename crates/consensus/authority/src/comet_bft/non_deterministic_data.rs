use bitcoin::consensus::encode::{self, Decodable, Encodable};

use std::io;
use thiserror::Error;

pub(crate) const NDD_VERSION_0: u16 = 0;
pub(crate) const NDD_VERSION_1: u16 = 1;

/// Errors that can occur when deserializing NonDeterministicData
#[derive(Debug, Error)]
pub(crate) enum NonDeterministicDataDeserializeError {
    #[error("I/O error")]
    /// I/O error
    Io(#[from] io::Error),
    #[error("invalid data format")]
    /// Invalid data format
    Decoding(#[from] encode::Error),
}

/// Type that encapsulates non-deterministic data needed for consensus
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NonDeterministicData {
    pub(crate) bitcoin_block_hash: bitcoin::hash_types::BlockHash,
    pub(crate) aggregated_public_key: secp256k1::PublicKey,
    // Version is sandwitched in the middle of the data b/c CometBFT does not support the first 16
    // bits of a tx being 0-bytes not sure if this is bug or feature
    pub(crate) version: u16,
    pub(crate) voting_bitmask: Option<u16>,
}

impl NonDeterministicData {
    pub(crate) fn version_default() -> u16 {
        NDD_VERSION_0
    }

    pub(crate) fn new(
        bitcoin_block_hash: bitcoin::hash_types::BlockHash,
        aggregated_public_key: secp256k1::PublicKey,
    ) -> Self {
        Self {
            version: NonDeterministicData::version_default(),
            bitcoin_block_hash,
            aggregated_public_key,
            voting_bitmask: None,
        }
    }

    #[allow(unused)]
    pub(crate) fn set_voting_bitmask(&mut self, voting_bitmask: u16) {
        self.voting_bitmask = Some(voting_bitmask);
    }

    #[allow(unused)]
    pub(crate) fn set_version(&mut self, version: u16) {
        self.version = version;
    }

    pub(crate) fn serialize(&self) -> Result<Vec<u8>, io::Error> {
        let mut writer = Vec::new();
        self.bitcoin_block_hash.consensus_encode(&mut writer)?;
        self.aggregated_public_key.serialize().consensus_encode(&mut writer)?;
        self.version.consensus_encode(&mut writer)?;
        if self.version >= NDD_VERSION_1 {
            if let Some(voting_bitmask) = self.voting_bitmask {
                voting_bitmask.consensus_encode(&mut writer)?;
            }
        }

        Ok(writer.to_vec())
    }

    pub(crate) fn deserialize(
        reader: &mut impl bitcoin::io::Read,
    ) -> Result<Self, NonDeterministicDataDeserializeError> {
        let bitcoin_block_hash = Decodable::consensus_decode(reader)?;

        let pk_bytes = <[u8; 33]>::consensus_decode(reader)?;
        let aggregated_public_key = secp256k1::PublicKey::from_slice(&pk_bytes).map_err(|e| {
            println!("Error: {:?}", e);
            encode::Error::ParseFailed("malformed aggregate public key")
        })?;
        let version = u16::consensus_decode(reader)?;
        let voting_bitmask = if version >= NDD_VERSION_1 {
            Some(u16::consensus_decode(reader)?)
        } else {
            None
        };

        Ok(Self { version, bitcoin_block_hash, aggregated_public_key, voting_bitmask })
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::{hashes::Hash, BlockHash};

    use super::*;

    #[test]
    fn test_non_deterministic_data_deserialize() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk: secp256k1::PublicKey = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        let ev = NonDeterministicData::new(bitcoin_block_hash, pk);
        let res = ev.serialize().unwrap();
        let mut reader = io::Cursor::new(res);
        let deserialized = NonDeterministicData::deserialize(&mut reader).unwrap();
        assert_eq!(deserialized, ev);
    }

    #[test]
    fn test_non_deterministic_data_deserialize_with_voting_bitmask() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk: secp256k1::PublicKey = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        let mut ev = NonDeterministicData::new(bitcoin_block_hash, pk);
        ev.set_version(NDD_VERSION_1);
        ev.set_voting_bitmask(1);
        let res = ev.serialize().unwrap();
        let mut reader = io::Cursor::new(res);
        let deserialized = NonDeterministicData::deserialize(&mut reader).unwrap();
        assert_eq!(deserialized, ev);
    }
}
