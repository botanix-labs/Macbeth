//! Non-deterministic data (NDD) used for extend cometbft blocks with botanix specific data.

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
}

// The default NDD version.
pub(crate) const VERSION_1: u16 = 1;

pub(crate) const LATEST_NDD_VERSION: u16 = VERSION_1;

/// Type that encapsulates non-deterministic data needed for consensus.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NonDeterministicData {
    pub(crate) version: u16,
    pub(crate) bitcoin_block_hash: bitcoin::hash_types::BlockHash,
    pub(crate) aggregated_public_key: secp256k1::PublicKey,
    pub(crate) block_fee_recipient_address: Address,
}

impl NonDeterministicData {
    /// Returns the version based on whether a fee recipient address is present.
    pub(crate) fn version(&self) -> u16 {
        self.version
    }

    /// Constructor for the NDD.
    pub(crate) fn new(
        bitcoin_block_hash: bitcoin::hash_types::BlockHash,
        aggregated_public_key: secp256k1::PublicKey,
        block_fee_recipient_address: Address,
    ) -> Self {
        Self {
            version: VERSION_1,
            bitcoin_block_hash,
            aggregated_public_key,
            block_fee_recipient_address,
        }
    }

    /// Serializes the non-deterministic data.
    pub(crate) fn serialize(&self) -> Result<Vec<u8>, io::Error> {
        let mut writer = Vec::new();
        self.bitcoin_block_hash.consensus_encode(&mut writer)?;
        self.aggregated_public_key.serialize().consensus_encode(&mut writer)?;
        self.version().consensus_encode(&mut writer)?;
        writer.write_all(self.block_fee_recipient_address.as_slice())?;

        Ok(writer)
    }

    /// Deserializes the non-deterministic data.
    pub(crate) fn deserialize(
        reader: &mut impl bitcoin::io::Read,
    ) -> Result<Self, NonDeterministicDataDeserializeError> {
        // Read the bitcoin block hash.
        let bitcoin_block_hash = Decodable::consensus_decode(reader)?;

        // Read the aggregated public key.
        let pk_bytes = <[u8; 33]>::consensus_decode(reader)?;
        let aggregated_public_key = secp256k1::PublicKey::from_slice(&pk_bytes)
            .map_err(|_e| encode::Error::ParseFailed("malformed aggregate public key"))?;

        // Read the version and conditionally read the address.
        let version = u16::consensus_decode(reader)?;

        // Read the block fee recipient address.
        let mut address_bytes = [0u8; 20];
        reader
            .read_exact(&mut address_bytes)
            .map_err(|_e| encode::Error::ParseFailed("malformed block fee recipient address"))?;
        let block_fee_recipient_address = Address::from(address_bytes);

        let this = Self {
            version,
            bitcoin_block_hash,
            aggregated_public_key,
            block_fee_recipient_address,
        };

        match version {
            VERSION_1 => {
                // The expected version 1 NDD.
                Ok(this)
            }
            _ => {
                // IMPORTANT: This is a versioning mechanism designed for
                // forward compatibility. We want to support new,
                // backwards-compatible versions with enhanced functionality
                // without requiring coordinated upgrades or hard-forks.
                //
                // When an older node encounters a newer, unknown version:
                // 1. It extracts the data it can understand based on the known structure
                // 2. It ignores any trailing data that may be present in newer versions
                // 3. It continues to function normally despite version differences
                //
                // By design, encountering an unknown version alone MUST NEVER
                // cause an error. The caller is responsible for validating
                // whether the returned NDD is appropriate for their specific
                // business logic requirements.
                Ok(this)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::{hashes::Hash, BlockHash};

    use super::*;

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
        assert_eq!(ndd.version, VERSION_1);
        assert_eq!(ndd.bitcoin_block_hash, bitcoin_block_hash);
        assert_eq!(ndd.aggregated_public_key, pk);
        assert_eq!(ndd.block_fee_recipient_address, block_fee_recipient_address);
    }

    #[test]
    fn test_non_deterministic_data_serde() {
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
        let res = ndd.serialize().unwrap();
        let mut reader = io::Cursor::new(res);

        let deserialized = NonDeterministicData::deserialize(&mut reader).unwrap();
        assert_eq!(deserialized.version, ndd.version);
        assert_eq!(deserialized.bitcoin_block_hash, ndd.bitcoin_block_hash);
        assert_eq!(deserialized.aggregated_public_key, ndd.aggregated_public_key);
        assert_eq!(deserialized.block_fee_recipient_address, ndd.block_fee_recipient_address);
    }

    #[test]
    /// Attempts to deserialize a NDD with an unknown version containing some
    /// tailing data.
    fn test_non_deterministic_data_serde_unknown_version() {
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

        // NOTE: The last 32 bytes (all 1's) are ignored.
        let bytes = hex::decode(
            "0000000000000000000000000000000000000000000000000000000000000000\
             039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d\
             6300\
             43c8bdcb9afebb1d834a7de18cc214a6fd1632d9\
             1111111111111111111111111111111111111111111111111111111111111111",
        )
        .unwrap();
        let mut reader = io::Cursor::new(bytes);
        let ndd = NonDeterministicData::deserialize(&mut reader).unwrap();

        assert_eq!(ndd.version, 99); // Version 99 is unknown
        assert_eq!(ndd.bitcoin_block_hash, bitcoin_block_hash);
        assert_eq!(ndd.aggregated_public_key, pk);
        assert_eq!(ndd.block_fee_recipient_address, block_fee_recipient_address);
    }

    #[test]
    /// Attempts to deserialize a NDD from a deprecated implementation that
    /// still used version 0.
    fn test_non_deterministic_data_serde_from_deprecated_impl() {
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

        let bytes = hex::decode(
            "0000000000000000000000000000000000000000000000000000000000000000\
             039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d\
             0100\
             43c8bdcb9afebb1d834a7de18cc214a6fd1632d9",
        )
        .unwrap();
        let mut reader = io::Cursor::new(bytes);
        let ndd = NonDeterministicData::deserialize(&mut reader).unwrap();

        assert_eq!(ndd.version, VERSION_1);
        assert_eq!(ndd.bitcoin_block_hash, bitcoin_block_hash);
        assert_eq!(ndd.aggregated_public_key, pk);
        assert_eq!(ndd.block_fee_recipient_address, block_fee_recipient_address);
    }
}
