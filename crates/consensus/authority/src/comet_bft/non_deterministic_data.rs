use bitcoin::consensus::encode::{self, Decodable, Encodable};

use std::io;
use thiserror::Error;

/// Errors that can occur when deserializing NonDeterministicData
#[derive(Debug, Error)]
pub(crate) enum NonDeterministicDataDeserializeError {
    #[error("I/O error")]
    /// I/O error
    Io(#[from] io::Error),
    #[error("invalid data format")]
    /// Invalid data format
    Decoding(#[from] encode::Error),
    #[error("invalid version")]
    /// Invalid NonDeterministicData, version
    InvalidVersion,
}

/// Type that encapsulates non-deterministic data needed for consensus
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NonDeterministicData {
    pub(crate) version: u16,
    pub(crate) bitcoin_block_hash: bitcoin::hash_types::BlockHash,
    pub(crate) aggregated_public_key: secp256k1::PublicKey,
}

impl NonDeterministicData {
    pub(crate) fn version_default() -> u16 {
        0
    }

    pub(crate) fn new(
        bitcoin_block_hash: bitcoin::hash_types::BlockHash,
        aggregated_public_key: secp256k1::PublicKey,
    ) -> Self {
        Self {
            version: NonDeterministicData::version_default(),
            bitcoin_block_hash,
            aggregated_public_key,
        }
    }

    pub(crate) fn serialize(&self) -> Result<Vec<u8>, io::Error> {
        let mut writer = Vec::new();
        self.bitcoin_block_hash.consensus_encode(&mut writer)?;
        self.aggregated_public_key.serialize().consensus_encode(&mut writer)?;
        self.version.consensus_encode(&mut writer)?;

        Ok(writer.to_vec())
    }

    pub(crate) fn deserialize(
        reader: &mut impl io::Read,
    ) -> Result<Self, NonDeterministicDataDeserializeError> {
        let bitcoin_block_hash = Decodable::consensus_decode(reader)?;

        let pk_bytes = <[u8; 33]>::consensus_decode(reader)?;
        let aggregated_public_key = secp256k1::PublicKey::from_slice(&pk_bytes).map_err(|e| {
            println!("Error: {:?}", e);
            encode::Error::ParseFailed("malformed aggregate public key")
        })?;
        let version = u16::consensus_decode(reader)?;
        if version != NonDeterministicData::version_default() {
            return Err(NonDeterministicDataDeserializeError::InvalidVersion);
        }

        Ok(Self { version, bitcoin_block_hash, aggregated_public_key })
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::{hashes::Hash, BlockHash};

    use super::*;

    #[test]
    fn test_version_default() {
        let version = NonDeterministicData::version_default();
        assert_eq!(version, 0);
    }

    #[test]
    fn test_non_deterministic_data_new() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        let ndd = NonDeterministicData::new(bitcoin_block_hash, pk);
        assert_eq!(ndd.version, NonDeterministicData::version_default());
        assert_eq!(ndd.bitcoin_block_hash, bitcoin_block_hash);
        assert_eq!(ndd.aggregated_public_key, pk);
    }

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
        assert_eq!(deserialized.version, ev.version);
        assert_eq!(deserialized.bitcoin_block_hash, ev.bitcoin_block_hash);
        assert_eq!(deserialized.aggregated_public_key, ev.aggregated_public_key);
    }
}
