use std::io;

use crate::constants::nums_secp256k1_pk;
use bitcoin::{
    consensus::encode::{self, Decodable, Encodable},
    hashes::Hash,
};
use revm_primitives::Address;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The version of the extra data header
pub const EXTRA_HEADER_VERSION: u32 = 0;
/// The version of the chain
pub const CHAIN_VERSION: u32 = 0;

/// Metadata fields that are included in the extra data header of botanix blocks
/// Federation members sign this data attesting to a new block and the set of authority signers
/// A block producer will sign `Hash(block_hash || extra_data_version || authority_signers ||
/// bitcoin_block_hash ... )` This sighash excludes the authority signature field.
/// Use `encode_into_without_signature` to serialize the extradata header with out the signature
/// field Note: the order of the struct properties is important for serialization/deserialization
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtraDataHeader {
    /// The version of the extra data header
    pub version: u32,
    /// Chain version that determines the valid chain
    /// this is a distinct field from chain id
    pub chain_version: u32,
    /// The hash of the bitcoin block that is sufficiently deep to prove pegins
    pub bitcoin_block_hash: bitcoin::hash_types::BlockHash,
    /// Aggregated public key
    pub aggregated_public_key: secp256k1::PublicKey,
    /// Block producer address
    pub block_producer_address: Address,
}

impl Default for ExtraDataHeader {
    // Note: default should never be used outside of tests
    fn default() -> Self {
        Self {
            version: EXTRA_HEADER_VERSION,
            chain_version: CHAIN_VERSION,
            bitcoin_block_hash: bitcoin::hash_types::BlockHash::all_zeros(),
            aggregated_public_key: nums_secp256k1_pk(),
            block_producer_address: Address::ZERO,
        }
    }
}

/// Errors that can occur when deserializing the extra data header
#[derive(Debug, Error)]
pub enum ExtraDataHeaderDeserializeError {
    #[error("I/O error")]
    /// I/O error
    Io(#[from] bitcoin::io::Error),
    #[error("invalid data format")]
    /// Invalid data format
    Decoding(#[from] encode::Error),
    #[error("invalid version")]
    /// Invalid EDH version
    InvalidVersion,
}

/// Errors that can occur when serializing the extra data header
#[derive(Debug, Error)]
pub enum ExtraDataHeaderSerializeError {
    #[error("Invalid format: {0}")]
    /// Invalid EDH format
    InvalidFormat(&'static str),
}

impl ExtraDataHeader {
    /// Create a new extra data header
    pub const fn new(
        version: u32,
        // Chain version that determines the valid chain
        chain_version: u32,
        // The hash of the bitcoin block that is sufficiently deep to prove pegins
        bitcoin_block_hash: bitcoin::hash_types::BlockHash,
        // Aggregated public key
        aggregated_public_key: secp256k1::PublicKey,
        // Block producer address
        block_producer_address: Address,
    ) -> Self {
        Self {
            version,
            chain_version,
            bitcoin_block_hash,
            aggregated_public_key,
            block_producer_address,
        }
    }

    /// Serialize the extra data header without the signature
    pub fn encode_into_without_signature(
        &self,
        writer: &mut impl bitcoin::io::Write,
    ) -> Result<(), io::Error> {
        self.version.consensus_encode(writer)?;
        self.chain_version.consensus_encode(writer)?;
        self.bitcoin_block_hash.consensus_encode(writer)?;
        self.aggregated_public_key.serialize().consensus_encode(writer)?;
        let block_producer_address_bytes = self.block_producer_address.0 .0;
        let _ = writer.write(&block_producer_address_bytes)?;

        Ok(())
    }

    /// Serialize the extra data header into the writer.
    pub fn encode_into(&self, writer: &mut impl bitcoin::io::Write) -> Result<(), io::Error> {
        self.encode_into_without_signature(writer)?;
        Ok(())
    }

    /// Serialize the extra data header
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        self.encode_into(&mut buf).expect("buffers produce no io errors");
        buf
    }

    /// Deserialize the extra data header
    pub fn deserialize(
        reader: &mut impl bitcoin::io::Read,
    ) -> Result<Self, ExtraDataHeaderDeserializeError> {
        let version = u32::consensus_decode(reader)?;
        // in the future you can deserialize specific versions of edh based on the version

        let chain_version = u32::consensus_decode(reader)?;
        let bitcoin_block_hash = Decodable::consensus_decode(reader)?;
        let pk_bytes = <[u8; 33]>::consensus_decode(reader)?;
        let aggregated_public_key = secp256k1::PublicKey::from_slice(&pk_bytes).map_err(|e| {
            println!("Error: {:?}", e);
            encode::Error::ParseFailed("malformed aggregate public key")
        })?;
        let mut block_producer_address_bytes: [u8; 20] = [0; 20];
        reader.read_exact(&mut block_producer_address_bytes)?;
        let block_producer_address = Address::from_slice(&block_producer_address_bytes);

        Ok(Self {
            version,
            chain_version,
            bitcoin_block_hash,
            aggregated_public_key,
            block_producer_address,
        })
    }

    /// returns the edh max size
    pub fn edh_max_size() -> usize {
        let edh = Self::default();
        edh.serialize().len()
    }

    /// returns the edh size
    pub fn edh_size(&self) -> usize {
        self.serialize().len()
    }
}

#[cfg(test)]
mod tests {
    use crate::constants::nums_secp256k1_pk;

    use super::*;
    use bitcoin::BlockHash;
    use rand::rngs::OsRng;
    use revm_primitives::hex;
    use secp256k1::Secp256k1;

    // Test case for creating a new ExtraDataHeader
    #[test]
    fn test_create_new_header() {
        let mainchain = BlockHash::hash(&[1, 2, 3]);

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            CHAIN_VERSION,
            mainchain,
            nums_secp256k1_pk(),
            Address::ZERO,
        );
        assert_eq!(header.version, EXTRA_HEADER_VERSION);
        assert_eq!(header.chain_version, CHAIN_VERSION);
        assert_eq!(header.bitcoin_block_hash, mainchain);
    }

    // Test case for edh max size
    #[test]
    fn check_max_edh_size() {
        assert!(ExtraDataHeader::edh_max_size() == 93);
    }

    // Test case for serializing without a signature
    #[test]
    fn serialize_without_signature() {
        let mut authority_signers = vec![];
        // Generate some pks
        let secp = Secp256k1::new();
        let (_, public_key) = secp.generate_keypair(&mut OsRng);
        authority_signers.push(public_key);
        let address = Address::random();

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            CHAIN_VERSION,
            BlockHash::hash(&[1]),
            nums_secp256k1_pk(),
            address,
        );
        let mut buf: Vec<u8> = vec![];
        header.encode_into_without_signature(&mut buf).unwrap();
        // serialize the same header
        let serialized =
            ExtraDataHeader::deserialize(&mut buf.as_slice()).expect("Deserialization");
        assert_eq!(serialized, header);
    }

    #[test]
    fn create_botanix_testnet_header() {
        let _pk1 = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        let _pk2 = secp256k1::PublicKey::from_slice(
            hex::decode("02bdc272b244f717604fffe659d2d98205d1e6764fdf453d1631f42c2db4d8d710")
                .unwrap()
                .as_slice(),
        )
        .unwrap();

        let extra_data_header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            CHAIN_VERSION,
            BlockHash::hash(&[1]),
            nums_secp256k1_pk(),
            Address::ZERO,
        );

        println!("serialized header: {}", hex::encode(extra_data_header.serialize()));
    }
}
