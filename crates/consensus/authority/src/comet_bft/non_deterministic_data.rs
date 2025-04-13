use crate::activation_manager;
use bitcoin::consensus::encode::{self, Decodable, Encodable};
use reth_primitives::Address;
use std::io::{self, Write};
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
    pub(crate) block_fee_recipient_address: Address,
}

impl NonDeterministicData {
    pub(crate) fn version_default() -> u16 {
        0
    }

    pub(crate) fn new(
        bitcoin_block_hash: bitcoin::hash_types::BlockHash,
        aggregated_public_key: secp256k1::PublicKey,
        block_fee_recipient_address: Address,
    ) -> Self {
        Self {
            version: NonDeterministicData::version_default(),
            bitcoin_block_hash,
            aggregated_public_key,
            block_fee_recipient_address,
        }
    }

    pub(crate) fn serialize(&self) -> Result<Vec<u8>, io::Error> {
        let mut writer = Vec::new();
        self.bitcoin_block_hash.consensus_encode(&mut writer)?;
        self.aggregated_public_key.serialize().consensus_encode(&mut writer)?;
        self.version.consensus_encode(&mut writer)?;
        writer.write_all(self.block_fee_recipient_address.as_slice())?;

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
        if version != NonDeterministicData::version_default() {
            return Err(NonDeterministicDataDeserializeError::InvalidVersion);
        }

        let mut address_bytes = [0u8; 20];
        reader
            .read_exact(&mut address_bytes)
            .map_err(|_e| encode::Error::ParseFailed("malformed block fee recipient address"))?;
        let block_fee_recipient_address = Address::from(address_bytes);

        Ok(Self { version, bitcoin_block_hash, aggregated_public_key, block_fee_recipient_address })
    }
}

/// Represents a validator's stance on a network upgrade proposal.
///
/// This payload is included in each block's non-deterministic data when a node is
/// configured to participate in the network upgrade voting process. It communicates
/// the validator's current position on a specific upgrade version.
///
/// # Fields
///
/// * `version` - The specific runtime version that this vote applies to.
///
/// * `vote` - The validator's explicit opinion on the upgrade (Aye/Nay/Absent).
///
/// * `is_compliant` - Indicates whether the validator is technically ready to process blocks with
///   the upgrade version. When `true`, the validator has the necessary software version and
///   configuration to handle the upgrade. This can be independent of the vote - a validator may
///   vote `Nay` but still be prepared to follow the network if the upgrade is adopted.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkUpgradePayload {
    pub version: activation_manager::RuntimeVersion,
    pub vote: activation_manager::Vote,
    pub is_compliant: bool,
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
        let block_fee_recipient_address =
            Address::parse_checksummed("0x43C8bDCb9AFeBB1D834A7de18CC214a6FD1632d9", None)
                .expect("valid address");
        let ndd = NonDeterministicData::new(bitcoin_block_hash, pk, block_fee_recipient_address);
        assert_eq!(ndd.version, NonDeterministicData::version_default());
        assert_eq!(ndd.bitcoin_block_hash, bitcoin_block_hash);
        assert_eq!(ndd.aggregated_public_key, pk);
        assert_eq!(ndd.block_fee_recipient_address, block_fee_recipient_address);
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
        let block_fee_recipient_address =
            Address::parse_checksummed("0x43C8bDCb9AFeBB1D834A7de18CC214a6FD1632d9", None)
                .expect("valid address");
        let ndd = NonDeterministicData::new(bitcoin_block_hash, pk, block_fee_recipient_address);
        let res = ndd.serialize().unwrap();
        let mut reader = io::Cursor::new(res);
        let deserialized = NonDeterministicData::deserialize(&mut reader).unwrap();
        assert_eq!(deserialized.version, ndd.version);
        assert_eq!(deserialized.bitcoin_block_hash, ndd.bitcoin_block_hash);
        assert_eq!(deserialized.aggregated_public_key, ndd.aggregated_public_key);
        assert_eq!(deserialized.block_fee_recipient_address, ndd.block_fee_recipient_address);
    }
}
