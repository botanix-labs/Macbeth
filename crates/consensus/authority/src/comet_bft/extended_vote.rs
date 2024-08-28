use std::io;

use bitcoin::consensus::encode::{self, Decodable, Encodable};
use thiserror::Error;

/// Errors that can occur when deserializing ExtendedVote
#[derive(Debug, Error)]
pub enum ExtendedVoteDeserializeError {
    #[error("I/O error")]
    /// I/O error
    Io(#[from] io::Error),
    #[error("invalid data format")]
    /// Invalid data format
    Decoding(#[from] encode::Error),
    #[error("invalid version")]
    /// Invalid ExtendedVote version
    InvalidVersion,
}

pub struct ExtendedVote {
    pub version: u16,
    pub bitcoin_block_hash: bitcoin::hash_types::BlockHash,
    pub aggregated_public_key: secp256k1::PublicKey,
}

impl ExtendedVote {
    pub fn version_default() -> u16 {
        0
    }

    pub fn new(
        bitcoin_block_hash: bitcoin::hash_types::BlockHash,
        aggregated_public_key: secp256k1::PublicKey,
    ) -> Self {
        Self { version: ExtendedVote::version_default(), bitcoin_block_hash, aggregated_public_key }
    }

    pub fn serialize(&self) -> Result<Vec<u8>, io::Error> {
        let mut writer = Vec::new();
        self.bitcoin_block_hash.consensus_encode(&mut writer)?;
        self.aggregated_public_key.serialize().consensus_encode(&mut writer)?;
        self.version.consensus_encode(&mut writer)?;

        Ok(writer.to_vec())
    }

    pub fn deserialize(reader: &mut impl io::Read) -> Result<Self, ExtendedVoteDeserializeError> {
        let bitcoin_block_hash = Decodable::consensus_decode(reader)?;

        let pk_bytes = <[u8; 33]>::consensus_decode(reader)?;
        let aggregated_public_key = secp256k1::PublicKey::from_slice(&pk_bytes).map_err(|e| {
            println!("Error: {:?}", e);
            encode::Error::ParseFailed("malformed aggregate public key")
        })?;
        let version = u16::consensus_decode(reader)?;
        if version != ExtendedVote::version_default() {
            return Err(ExtendedVoteDeserializeError::InvalidVersion);
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
        let version = ExtendedVote::version_default();
        assert_eq!(version, 0);
    }

    #[test]
    fn test_extended_vote_new() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        let ev = ExtendedVote::new(bitcoin_block_hash, pk);
        assert_eq!(ev.version, ExtendedVote::version_default());
        assert_eq!(ev.bitcoin_block_hash, bitcoin_block_hash);
        assert_eq!(ev.aggregated_public_key, pk);
    }

    #[test]
    fn test_extended_vote_serialize_deserialize() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk: secp256k1::PublicKey = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        let ev = ExtendedVote::new(bitcoin_block_hash, pk);
        let res = ev.serialize().unwrap();
        let mut reader = io::Cursor::new(res);
        let deserialized = ExtendedVote::deserialize(&mut reader).unwrap();
        assert_eq!(deserialized.version, ev.version);
        assert_eq!(deserialized.bitcoin_block_hash, ev.bitcoin_block_hash);
        assert_eq!(deserialized.aggregated_public_key, ev.aggregated_public_key);
    }
}
