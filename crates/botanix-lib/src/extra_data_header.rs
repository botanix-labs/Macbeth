use std::io;

use bitcoin::{
    consensus::encode::{self, Decodable, Encodable},
    secp256k1,
};
use secp256k1::{ecdsa::RecoveryId, hashes::Hash};
use thiserror::Error;

use lazy_static;

lazy_static::lazy_static! {
    /// Signature Recovery Id
    pub static ref ECDSA_RECOVERY_ID: RecoveryId = RecoveryId::from_i32(1i32).expect("recovery id");
}

const EXTRA_HEADER_VERSION: u32 = 0;
const HAS_AUTHORTIES_POS: u8 = 0;
const HAS_VOTE_POS: u8 = 1;
const HAS_SIGNATURE_POS: u8 = 2;

/// Metadata fields that are included in the extra data header of botanix blocks
/// Federation members sign this data attesting to a new block and the set of authority signers
/// A block producer will sign `Hash(block_hash || extra_data_version || authority_signers ||
/// authority_vote || bitcoin_block_hash ... )` This sighash excludes the authority signature field.
/// Use `encode_into_without_signature` to serialize the extradata header with out the signature
/// field Note: the order of the struct properties is important for serialization/deserialization
#[derive(Debug, Clone)]
pub struct ExtraDataHeader {
    pub version: u32,
    pub optional_fields: u8,
    pub authority_signers: Option<Vec<secp256k1::PublicKey>>,
    pub authority_vote: Option<secp256k1::PublicKey>,
    pub bitcoin_block_hash: bitcoin::hash_types::BlockHash,
    // TODO add bitcoin fee
    pub authority_signature: Option<secp256k1::ecdsa::RecoverableSignature>,
}

impl Default for ExtraDataHeader {
    fn default() -> Self {
        Self {
            version: EXTRA_HEADER_VERSION,
            optional_fields: 0,
            authority_signers: None,
            authority_vote: None,
            bitcoin_block_hash: bitcoin::hash_types::BlockHash::all_zeros(),
            authority_signature: None,
        }
    }
}

/// Errors that can occur when deserializing the extra data header
#[derive(Debug, Error)]
pub enum ExtraDataHeaderDeserialzeError {
    #[error("I/O error")]
    Io(#[from] io::Error),
    #[error("invalid data format")]
    Decoding(#[from] encode::Error),
    #[error("invalid version")]
    InvalidVersion,
}

/// Errors that can occur when validating the authority signature
#[derive(Debug, Error, PartialEq)]
pub enum ValidateAuthoritySignatureError {
    #[error("invalid signature")]
    InvalidSignature,
    #[error("invalid message")]
    InvalidMessage,
    #[error("missing Signature")]
    MissingSignature,
}

/// Errors that can occur when serializing the extra data header
#[derive(Debug, Error)]
pub enum ExtraDataHeaderSerializeError {
    #[error("Signature missing")]
    InvalidFormat(&'static str),
}

impl ExtraDataHeader {
    pub fn new(
        version: u32,
        // This field is only optional b/c the block producer will need to sign the extra header
        // data without a signature appended at the end
        authority_signature: Option<secp256k1::ecdsa::RecoverableSignature>,
        // Optional set of authority signers. Non-optional during a epoch block. This should be
        // validated by consensus
        authority_signers: Option<Vec<secp256k1::PublicKey>>,
        authority_vote: Option<secp256k1::PublicKey>,
        bitcoin_block_hash: bitcoin::hash_types::BlockHash,
    ) -> Self {
        let mut optional_fields = 0u8;
        if authority_signers.is_some() {
            optional_fields |= 1 << HAS_AUTHORTIES_POS;
        }
        if authority_vote.is_some() {
            optional_fields |= 1 << HAS_VOTE_POS;
        }
        if authority_signature.is_some() {
            optional_fields |= 1 << HAS_SIGNATURE_POS;
        }

        Self {
            version,
            authority_signers,
            authority_vote,
            bitcoin_block_hash,
            authority_signature,
            optional_fields,
        }
    }

    pub fn set_optional_fields_bitmask(
        &mut self,
    ) -> () {
        let mut optional_fields = 0u8;
        if self.authority_signers.is_some() {
            optional_fields |= 1 << HAS_AUTHORTIES_POS;
        }
        if self.authority_vote.is_some() {
            optional_fields |= 1 << HAS_VOTE_POS;
        }
        if self.authority_signature.is_some() {
            optional_fields |= 1 << HAS_SIGNATURE_POS;
        }

        self.optional_fields = optional_fields;
    }

    pub fn authority_vote(&self) -> Option<secp256k1::PublicKey> {
        self.authority_vote
    }

    pub fn encode_into_without_signature(
        &self,
        writer: &mut impl io::Write,
    ) -> Result<(), io::Error> {
        self.version.consensus_encode(writer)?;
        self.optional_fields.consensus_encode(writer)?;
        self.bitcoin_block_hash.consensus_encode(writer)?;

        if let Some(authorities) = &self.authority_signers {
            (authorities.len() as u32).consensus_encode(writer)?;
            for k in authorities {
                k.serialize().consensus_encode(writer)?;
            }
        }

        if let Some(vote) = self.authority_vote {
            vote.serialize().consensus_encode(writer)?;
        }

        Ok(())
    }

    /// Serialize the extra data header into the writer.
    pub fn encode_into(&self, writer: &mut impl io::Write) -> Result<(), io::Error> {
        self.encode_into_without_signature(writer)?;
        if let Some(sig) = self.authority_signature {
            writer.write_all(&sig.serialize_compact().1[..])?;
        }
        Ok(())
    }

    /// Serialize the extra data header
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        self.encode_into(&mut buf).expect("buffers produce no io errors");
        buf
    }

    pub fn deserialize(reader: &mut impl io::Read) -> Result<Self, ExtraDataHeaderDeserialzeError> {
        let version = u32::consensus_decode(reader)?;
        if version > EXTRA_HEADER_VERSION {
            return Err(ExtraDataHeaderDeserialzeError::InvalidVersion)
        }
        let optional_fields = u8::consensus_decode(reader)?;
        let bitcoin_block_hash = Decodable::consensus_decode(reader)?;

        // Everything past the blockhash is optional and can be empty
        // use the optional bitmask field

        let authority_signers = if optional_fields & (1u8 << HAS_AUTHORTIES_POS) != 0 {
            let signer_len = u32::consensus_decode(reader)?;
            let mut signers = Vec::with_capacity(signer_len as usize);
            for _ in 0..signer_len {
                let bytes: [u8; 33] = <[u8; 33]>::consensus_decode(reader)?;
                let pk = secp256k1::PublicKey::from_slice(&bytes)
                    .map_err(|_| encode::Error::ParseFailed("invalid signer public key"))?;
                signers.push(pk);
            }
            Some(signers)
        } else {
            None
        };

        let authority_vote = if optional_fields & (1u8 << HAS_VOTE_POS) != 0 {
            let bytes: [u8; 33] = <[u8; 33]>::consensus_decode(reader)?;
            let pk = secp256k1::PublicKey::from_slice(&bytes)
                .map_err(|_| encode::Error::ParseFailed("invalid signer public key"))?;
            Some(pk)
        } else {
            None
        };

        let signature = if optional_fields & (1u8 << HAS_SIGNATURE_POS) != 0 {
            let mut buf = [0; 64];
            reader.read_exact(&mut buf)?;
            Some(
                secp256k1::ecdsa::RecoverableSignature::from_compact(&buf, *ECDSA_RECOVERY_ID)
                    .map_err(|_| encode::Error::ParseFailed("Invalid signature"))?,
            )
        } else {
            None
        };

        Ok(Self {
            version,
            optional_fields,
            bitcoin_block_hash,
            authority_signers,
            authority_vote,
            authority_signature: signature,
        })
    }

    pub fn validate_authority_signature(
        self,
        message: &Vec<u8>,
        authority_signers: &Vec<secp256k1::PublicKey>,
    ) -> Result<(), ValidateAuthoritySignatureError> {
        if self.authority_signature.is_none() {
            return Err(ValidateAuthoritySignatureError::MissingSignature)
        }

        let msg = secp256k1::Message::from_slice(message.as_slice())
            .map_err(|_| ValidateAuthoritySignatureError::InvalidMessage)?;

        if authority_signers.into_iter().any(|signer| {
            self.authority_signature
                .expect("signature exists")
                .to_standard()
                .verify(&msg, &signer)
                .is_ok()
        }) {
            return Ok(())
        }

        Err(ValidateAuthoritySignatureError::InvalidSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secp256k1::{
        hashes::{sha256, Hash},
        rand::rngs::OsRng,
        Message, Secp256k1,
    };

    // Test case for creating a new ExtraDataHeader
    #[test]
    fn test_create_new_header() {
        let authority_signers = vec![];
        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            None,
            Some(authority_signers.clone()),
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
        );
        assert_eq!(header.version, EXTRA_HEADER_VERSION);
        assert_eq!(header.authority_signature, None);
        assert_eq!(header.authority_signers, Some(authority_signers));
        assert_eq!(header.authority_vote, None);
        assert_eq!(header.bitcoin_block_hash, bitcoin::hash_types::BlockHash::all_zeros());
    }

    // Test case for serializing without a signature
    #[test]
    fn encode_into_without_signature() {
        let mut authority_signers = vec![];
        // Generate some pks
        let secp = Secp256k1::new();
        let (_, public_key) = secp.generate_keypair(&mut OsRng);
        authority_signers.push(public_key);

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            None,
            Some(authority_signers),
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
        );
        let mut buf: Vec<u8> = vec![];
        header.encode_into_without_signature(&mut buf).unwrap();
        println!("buf: {:?}", hex::encode(&buf));
        // Check version
        assert_eq!(buf[0..4], vec![0u8, 0u8, 0u8, 0u8].as_slice().to_owned());
        // Check optional bitmask
        assert_eq!(buf[4..5], vec![1u8].as_slice().to_owned());
        // Check the bitcoin block hash
        let bitcoin_block_hash: bitcoin::hash_types::BlockHash =
            bitcoin::consensus::deserialize(&buf[5..37]).expect("a bitcoin block hash");
        assert_eq!(bitcoin_block_hash, bitcoin::hash_types::BlockHash::all_zeros());
        // Check length of authority list
        assert_eq!(buf[37..41], vec![1u8, 0u8, 0u8, 0u8].as_slice().to_owned());
        // Check the pk
        let maybe_pk = buf[41..74].to_vec();
        let pk = secp256k1::PublicKey::from_slice(&maybe_pk.as_slice()).expect("a public key");
        // Check the public key is the same as one provided
        assert_eq!(pk, public_key);
    }

    // Test case for serializing with a signature
    #[test]
    fn test_serialize_with_signature() {
        let mut authority_signers = vec![];
        // Generate some pks
        let secp = Secp256k1::new();
        let (secret_key, public_key) = secp.generate_keypair(&mut OsRng);
        authority_signers.push(public_key);

        let message = Message::from_hashed_data::<sha256::Hash>("Hello World!".as_bytes());
        let signature = secp.sign_ecdsa_recoverable(&message, &secret_key);

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            Some(signature),
            Some(authority_signers),
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
        );
        let serialized = header.serialize();

        let deserialized_header = ExtraDataHeader::deserialize(&mut serialized.as_slice())
            .expect("Deserialization failed");
        let authority_signers = deserialized_header.authority_signers.expect("authority signers");

        assert_eq!(deserialized_header.version, 0);
        assert_eq!(authority_signers.len(), 1);
        assert_eq!(authority_signers[0], public_key);
        assert_eq!(
            deserialized_header.bitcoin_block_hash,
            bitcoin::hash_types::BlockHash::all_zeros()
        );
        assert_eq!(deserialized_header.authority_vote, None);

        assert_eq!(deserialized_header.authority_signature.is_some(), true);

        assert_eq!(
            deserialized_header.authority_signature.unwrap().to_standard(),
            signature.to_standard()
        );

        deserialized_header
            .authority_signature
            .unwrap()
            .to_standard()
            .verify(&message, &public_key)
            .expect("signature from same pk");
    }

    #[test]
    fn test_serialize_with_vote() {
        let mut authority_signers = vec![];
        // Generate some pks
        let secp = Secp256k1::new();
        let (secret_key, public_key) = secp.generate_keypair(&mut OsRng);
        authority_signers.push(public_key);

        let message = Message::from_hashed_data::<sha256::Hash>("Hello World!".as_bytes());
        let signature = secp.sign_ecdsa_recoverable(&message, &secret_key);

        let (_, pubkey_to_vote) = secp.generate_keypair(&mut OsRng);

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            Some(signature),
            Some(authority_signers),
            Some(pubkey_to_vote),
            bitcoin::hash_types::BlockHash::all_zeros(),
        );

        let serialized = header.serialize();

        let deserialized_header = ExtraDataHeader::deserialize(&mut serialized.as_slice())
            .expect("Deserialization failed");

        let authorities = deserialized_header.authority_signers.expect("authority signers");

        assert_eq!(deserialized_header.version, 0);
        assert_eq!(authorities.len(), 1);
        assert_eq!(authorities[0], public_key);
        assert_eq!(
            deserialized_header.bitcoin_block_hash,
            bitcoin::hash_types::BlockHash::all_zeros()
        );
        assert_eq!(deserialized_header.authority_vote, Some(pubkey_to_vote));

        assert_eq!(deserialized_header.authority_signature.is_some(), true);

        assert_eq!(
            deserialized_header.authority_signature.unwrap().to_standard(),
            signature.to_standard()
        );

        deserialized_header
            .authority_signature
            .unwrap()
            .to_standard()
            .verify(&message, &public_key)
            .expect("signature from same pk");
    }

    #[test]
    fn test_serialize_with_out_authorities() {
        // Generate some pks
        let secp = Secp256k1::new();
        let (secret_key, public_key) = secp.generate_keypair(&mut OsRng);

        let message = Message::from_hashed_data::<sha256::Hash>("Hello World!".as_bytes());
        let signature = secp.sign_ecdsa_recoverable(&message, &secret_key);

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            Some(signature),
            None,
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
        );

        let serialized = header.serialize();

        let deserialized_header = ExtraDataHeader::deserialize(&mut serialized.as_slice())
            .expect("Deserialization failed");

        assert_eq!(deserialized_header.version, 0);
        assert_eq!(deserialized_header.authority_signers, None);
        assert_eq!(
            deserialized_header.bitcoin_block_hash,
            bitcoin::hash_types::BlockHash::all_zeros()
        );
        assert_eq!(deserialized_header.authority_vote, None);
        assert_eq!(deserialized_header.authority_signature.is_some(), true);
        assert_eq!(
            deserialized_header.authority_signature.unwrap().to_standard(),
            signature.to_standard()
        );

        deserialized_header
            .authority_signature
            .unwrap()
            .to_standard()
            .verify(&message, &public_key)
            .expect("signature from same pk");
    }

    // Test case for validating with a signature
    #[test]
    fn test_validate_authority_signature() {
        let mut authority_signers = vec![];
        let secp = Secp256k1::new();
        let (secret_key, public_key) = secp.generate_keypair(&mut OsRng);
        authority_signers.push(public_key);

        let hello_world_hash = sha256::Hash::hash("Hello world!".as_bytes());
        let message = Message::from(hello_world_hash);
        let signature = secp.sign_ecdsa_recoverable(&message, &secret_key);

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            Some(signature),
            Some(authority_signers.clone()),
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
        );

        header
            .validate_authority_signature(
                &hello_world_hash.as_byte_array().to_vec(),
                &authority_signers,
            )
            .unwrap()
    }

    // Test case for validating with an invalid signature
    #[test]
    fn test_validate_authority_signature_with_invalid_signature() {
        let mut authority_signers = vec![];
        let secp = Secp256k1::new();
        let (secret_key, public_key) = secp.generate_keypair(&mut OsRng);
        authority_signers.push(public_key);

        let hello_world_hash = sha256::Hash::hash("Hello world!".as_bytes());
        let message = Message::from(hello_world_hash);
        let signature = secp.sign_ecdsa_recoverable(&message, &secret_key);

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            Some(signature),
            Some(authority_signers.clone()),
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
        );
        let invalid_hash = sha256::Hash::hash("Not hello world!".as_bytes());
        let result = header.validate_authority_signature(
            &invalid_hash.as_byte_array().to_vec(),
            &authority_signers,
        );
        assert_eq!(result.unwrap_err(), ValidateAuthoritySignatureError::InvalidSignature)
    }

    // Test case for validating without a signature
    #[test]
    fn test_validate_authority_signature_without_signature() {
        let mut authority_signers = vec![];
        let secp = Secp256k1::new();
        let (_, public_key) = secp.generate_keypair(&mut OsRng);
        authority_signers.push(public_key);

        let header_without_signature = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            None,
            Some(authority_signers.clone()),
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
        );

        let message = vec![0u8; 32];

        let result =
            header_without_signature.validate_authority_signature(&message, &authority_signers);
        assert_eq!(result.unwrap_err(), ValidateAuthoritySignatureError::MissingSignature);
    }

    #[test]
    fn creates_correct_optional_fields_bitmask() {
        let mut authority_signers = vec![];
        let secp = Secp256k1::new();
        let (secret_key, public_key) = secp.generate_keypair(&mut OsRng);
        authority_signers.push(public_key);

        let message = Message::from_hashed_data::<sha256::Hash>("Hello World!".as_bytes());
        let signature = secp.sign_ecdsa_recoverable(&message, &secret_key);

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            Some(signature),
            Some(authority_signers),
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
        );

        let optional_fields = header.optional_fields;
        assert_ne!(optional_fields, 0);
        assert_eq!(optional_fields & (1u8 << HAS_AUTHORTIES_POS), 1u8 << HAS_AUTHORTIES_POS,);
        assert_eq!(optional_fields & (1u8 << HAS_VOTE_POS), 0);
        assert_eq!(optional_fields & (1u8 << HAS_SIGNATURE_POS), 1u8 << HAS_SIGNATURE_POS);
    }

    #[test]
    fn serialize_without_any_authorities() {
        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            None,
            None,
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
        );

        let serialized = header.serialize();

        let deserialized_header = ExtraDataHeader::deserialize(&mut serialized.as_slice())
            .expect("Deserialization failed");

        assert_eq!(deserialized_header.version, 0);
        assert_eq!(deserialized_header.optional_fields, 0);
        assert_eq!(deserialized_header.authority_signers, None);
        assert_eq!(
            deserialized_header.bitcoin_block_hash,
            bitcoin::hash_types::BlockHash::all_zeros()
        );
        assert_eq!(deserialized_header.authority_vote, None);
        assert_eq!(deserialized_header.authority_signature, None);
    }

    #[test]
    fn create_botanix_testnet_header() {
        let pk1 = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        let pk2 = secp256k1::PublicKey::from_slice(
            hex::decode("02bdc272b244f717604fffe659d2d98205d1e6764fdf453d1631f42c2db4d8d710")
                .unwrap()
                .as_slice(),
        )
        .unwrap();

        let extra_data_header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            None,
            Some(vec![pk1, pk2]),
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
        );

        println!("serialized header: {}", hex::encode(extra_data_header.serialize()));
    }
}
