use std::io;

use bitcoin::{
    consensus::encode::{self, Decodable, Encodable},
    hashes::Hash,
    secp256k1, witness,
};

use secp256k1::ecdsa::{RecoverableSignature, RecoveryId};
use thiserror::Error;

pub const EXTRA_HEADER_VERSION: u32 = 0;
const HAS_AUTHORTIES_POS: u8 = 0;
const HAS_VOTE_POS: u8 = 1;
const HAS_SIGNATURE_POS: u8 = 2;
const HAS_WITNESS_DATA_POS: u8 = 3;

/// Metadata fields that are included in the extra data header of botanix blocks
/// Federation members sign this data attesting to a new block and the set of authority signers
/// A block producer will sign `Hash(block_hash || extra_data_version || authority_signers ||
/// authority_vote || bitcoin_block_hash ... )` This sighash excludes the authority signature field.
/// Use `encode_into_without_signature` to serialize the extradata header with out the signature
/// field Note: the order of the struct properties is important for serialization/deserialization
#[derive(Debug, Clone, PartialEq)]
pub struct ExtraDataHeader {
    pub version: u32,
    pub optional_fields: u8,
    pub authority_signers: Option<Vec<secp256k1::PublicKey>>,
    pub authority_vote: Option<secp256k1::PublicKey>,
    pub witness_data: Option<Vec<witness::Witness>>,
    pub bitcoin_block_hash: bitcoin::hash_types::BlockHash,
    pub utxo_commitment: [u8; 32],
    // TODO add bitcoin fee
    pub authority_signatures: Option<Vec<secp256k1::ecdsa::RecoverableSignature>>,
}

impl Default for ExtraDataHeader {
    fn default() -> Self {
        Self {
            version: EXTRA_HEADER_VERSION,
            optional_fields: 0,
            authority_signers: None,
            authority_vote: None,
            witness_data: None,
            bitcoin_block_hash: bitcoin::hash_types::BlockHash::all_zeros(),
            utxo_commitment: [0; 32],
            authority_signatures: None,
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
    #[error("missing signature")]
    MissingSignature,
    #[error("quorum of signatures missing")]
    QuorumMissing,
    #[error("cannot find signer at index: {0}")]
    InvalidSignerIndex(usize),
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
        authority_signatures: Option<Vec<secp256k1::ecdsa::RecoverableSignature>>,
        // Optional set of authority signers. Non-optional during a epoch block. This should be
        // validated by consensus
        authority_signers: Option<Vec<secp256k1::PublicKey>>,
        authority_vote: Option<secp256k1::PublicKey>,
        // Optional witness data. Non-optional during a epoch block. This should be validated by
        // consensus
        witness_data: Option<Vec<witness::Witness>>,
        // The hash of the bitcoin block that is sufficiently deep to prove pegins
        bitcoin_block_hash: bitcoin::hash_types::BlockHash,
        // The commitment to the UTXO set. i.e utxos that are spendable for pegouts
        utxo_commitment: [u8; 32],
    ) -> Self {
        let mut optional_fields = 0u8;
        if authority_signers.is_some() {
            optional_fields |= 1 << HAS_AUTHORTIES_POS;
        }
        if authority_vote.is_some() {
            optional_fields |= 1 << HAS_VOTE_POS;
        }
        if authority_signatures.is_some() {
            optional_fields |= 1 << HAS_SIGNATURE_POS;
        }
        if witness_data.is_some() {
            optional_fields |= 1 << HAS_WITNESS_DATA_POS;
        }

        Self {
            version,
            authority_signers,
            authority_vote,
            witness_data,
            bitcoin_block_hash,
            utxo_commitment,
            authority_signatures,
            optional_fields,
        }
    }

    pub fn set_signature(&mut self, signature: Vec<RecoverableSignature>) {
        self.authority_signatures = Some(signature);
        self.set_optional_fields_bitmask();
    }

    pub fn add_signature(&mut self, signature: RecoverableSignature) {
        let mut current_signatures = self.authority_signatures.clone().unwrap_or(vec![]);

        // Check if this signature already exists in the list
        if current_signatures.contains(&signature) {
            return;
        }
        current_signatures.push(signature);
        self.authority_signatures = Some(current_signatures);
        self.set_optional_fields_bitmask();
    }

    pub fn set_optional_fields_bitmask(&mut self) {
        let mut optional_fields = 0u8;
        if self.authority_signers.is_some() {
            optional_fields |= 1 << HAS_AUTHORTIES_POS;
        }
        if self.authority_vote.is_some() {
            optional_fields |= 1 << HAS_VOTE_POS;
        }
        if self.authority_signatures.is_some() {
            optional_fields |= 1 << HAS_SIGNATURE_POS;
        }
        if self.witness_data.is_some() {
            optional_fields |= 1 << HAS_WITNESS_DATA_POS;
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
        self.utxo_commitment.consensus_encode(writer)?;

        if let Some(authorities) = &self.authority_signers {
            (authorities.len() as u32).consensus_encode(writer)?;
            for k in authorities {
                k.serialize().consensus_encode(writer)?;
            }
        }

        if let Some(vote) = self.authority_vote {
            vote.serialize().consensus_encode(writer)?;
        }

        if let Some(witness_data) = &self.witness_data {
            (witness_data.len() as u32).consensus_encode(writer)?;
            for w in witness_data {
                w.consensus_encode(writer)?;
            }
        }

        Ok(())
    }

    /// Serialize the extra data header into the writer.
    pub fn encode_into(&self, writer: &mut impl io::Write) -> Result<(), io::Error> {
        self.encode_into_without_signature(writer)?;
        if let Some(sigs) = &self.authority_signatures {
            // Write length of signatures
            let len = sigs.len() as u32;
            (len).consensus_encode(writer)?;
            for sig in sigs {
                let (recovery_id, sig) = &sig.serialize_compact();
                let _ = i32::consensus_encode(&recovery_id.to_i32(), writer);
                writer.write_all(&sig[..])?;
            }
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
            return Err(ExtraDataHeaderDeserialzeError::InvalidVersion);
        }
        let optional_fields = u8::consensus_decode(reader)?;
        let bitcoin_block_hash = Decodable::consensus_decode(reader)?;
        let utxo_commitment = Decodable::consensus_decode(reader)?;

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

        let witness_data = if optional_fields & (1u8 << HAS_WITNESS_DATA_POS) != 0 {
            let witness_len = u32::consensus_decode(reader)?;
            let mut witness_data = Vec::with_capacity(witness_len as usize);
            for _ in 0..witness_len {
                let witness = witness::Witness::consensus_decode(reader)?;
                witness_data.push(witness);
            }
            Some(witness_data)
        } else {
            None
        };

        let signatures = if optional_fields & (1u8 << HAS_SIGNATURE_POS) != 0 {
            let mut sigs = vec![];
            let signature_len = u32::consensus_decode(reader)?;
            for _ in 0..signature_len {
                let recovery_id = RecoveryId::from_i32(i32::consensus_decode(reader)?).unwrap();
                let mut buf = [0; 64];
                reader.read_exact(&mut buf)?;
                let signature =
                    secp256k1::ecdsa::RecoverableSignature::from_compact(&buf, recovery_id)
                        .map_err(|_| encode::Error::ParseFailed("Invalid signature"))?;
                sigs.push(signature);
            }

            Some(sigs)
        } else {
            None
        };

        Ok(Self {
            version,
            optional_fields,
            bitcoin_block_hash,
            utxo_commitment,
            authority_signers,
            authority_vote,
            witness_data,
            authority_signatures: signatures,
        })
    }

    pub fn validate_single_authority_signature(
        &self,
        message: &Vec<u8>,
        authority_signers: &[secp256k1::PublicKey],
    ) -> Result<(), ValidateAuthoritySignatureError> {
        if self.authority_signatures.is_none() {
            return Err(ValidateAuthoritySignatureError::MissingSignature);
        }
        let msg = secp256k1::Message::from_slice(message.as_slice())
            .map_err(|_| ValidateAuthoritySignatureError::InvalidMessage)?;

        // Just validating the first signature
        let sig = self.authority_signatures.as_ref().expect("is some")[0];
        let recovered_pk =
            sig.recover(&msg).map_err(|_| ValidateAuthoritySignatureError::InvalidSignature)?;

        // find pk in authority signers
        for signer in authority_signers {
            if signer == &recovered_pk && sig.to_standard().verify(&msg, signer).is_ok() {
                return Ok(());
            }
        }

        Err(ValidateAuthoritySignatureError::InvalidSignature)
    }

    /// Validates all signatures present on the edh
    pub fn validate_authorities_signatures(
        self,
        message: &Vec<u8>,
        authority_signers: &[secp256k1::PublicKey],
    ) -> Result<(), ValidateAuthoritySignatureError> {
        if self.authority_signatures.is_none() {
            return Err(ValidateAuthoritySignatureError::MissingSignature);
        }

        let msg = secp256k1::Message::from_slice(message.as_slice())
            .map_err(|_| ValidateAuthoritySignatureError::InvalidMessage)?;

        let mut signer_count = 0;
        for sig in self.authority_signatures.expect("is some") {
            let recovered_pk =
                sig.recover(&msg).map_err(|_| ValidateAuthoritySignatureError::InvalidSignature)?;
            for signer in authority_signers {
                if signer == &recovered_pk && sig.to_standard().verify(&msg, signer).is_ok() {
                    signer_count += 1;
                }
            }
        }
        // TODO (armins) change this to be a quorum of signatures
        if signer_count >= authority_signers.len() {
            return Ok(());
        }
        Err(ValidateAuthoritySignatureError::InvalidSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;
    use secp256k1::{
        hashes::sha256,
        rand::{rngs::OsRng, thread_rng, RngCore},
        Message, Secp256k1,
    };

    // Test case for creating a new ExtraDataHeader
    #[test]
    fn test_create_new_header() {
        let mut rand = thread_rng();
        let mut random_32_bytes: [u8; 32] = [0u8; 32];
        rand.fill_bytes(&mut random_32_bytes);

        let authority_signers = vec![];
        let witness_data = vec![witness::Witness::default()];
        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            None,
            Some(authority_signers.clone()),
            None,
            Some(witness_data.clone()),
            bitcoin::hash_types::BlockHash::all_zeros(),
            random_32_bytes,
        );
        assert_eq!(header.version, EXTRA_HEADER_VERSION);
        assert_eq!(header.authority_signatures, None);
        assert_eq!(header.authority_signers, Some(authority_signers));
        assert_eq!(header.authority_vote, None);
        assert_eq!(header.witness_data, Some(witness_data));
        assert_eq!(header.bitcoin_block_hash, bitcoin::hash_types::BlockHash::all_zeros());
        assert_eq!(header.utxo_commitment, random_32_bytes);
    }

    // Test case for serializing without a signature
    #[test]
    fn encode_into_without_signature() {
        let mut authority_signers = vec![];
        // Generate some pks
        let secp = Secp256k1::new();
        let (_, public_key) = secp.generate_keypair(&mut OsRng);
        authority_signers.push(public_key);
        let witness_data = vec![witness::Witness::default()];

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            None,
            Some(authority_signers),
            None,
            Some(witness_data),
            bitcoin::hash_types::BlockHash::all_zeros(),
            [0u8; 32],
        );
        let mut buf: Vec<u8> = vec![];
        header.encode_into_without_signature(&mut buf).unwrap();
        println!("{:?}", buf);
        // serialize the same header
        let serialized =
            ExtraDataHeader::deserialize(&mut buf.as_slice()).expect("Deserialization");
        assert_eq!(serialized, header);
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
            Some(vec![signature]),
            Some(authority_signers),
            None,
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
            [0u8; 32],
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
        assert_eq!(deserialized_header.witness_data, None);
        assert_eq!(deserialized_header.authority_signatures.clone().unwrap(), vec![signature]);

        let recovered_pk = signature.recover(&message).unwrap();
        assert_eq!(recovered_pk, public_key);

        deserialized_header.authority_signatures.unwrap()[0]
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
        let witness_data = vec![witness::Witness::default()];

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            Some(vec![signature]),
            Some(authority_signers),
            Some(pubkey_to_vote),
            Some(witness_data.clone()),
            bitcoin::hash_types::BlockHash::all_zeros(),
            [0u8; 32],
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

        assert_eq!(deserialized_header.witness_data, Some(witness_data));

        assert!(deserialized_header.authority_signatures.is_some());

        assert_eq!(
            deserialized_header.authority_signatures.clone().unwrap()[0].to_standard(),
            signature.to_standard()
        );

        deserialized_header.authority_signatures.unwrap()[0]
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
            Some(vec![signature]),
            None,
            None,
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
            [0u8; 32],
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
        assert_eq!(deserialized_header.witness_data, None);
        assert_eq!(deserialized_header.authority_signatures.is_some(), true);
        assert_eq!(
            deserialized_header.authority_signatures.clone().unwrap()[0].to_standard(),
            signature.to_standard()
        );

        deserialized_header.authority_signatures.unwrap()[0]
            .to_standard()
            .verify(&message, &public_key)
            .expect("signature from same pk");
    }

    // Test case for validating with a signature
    #[test]
    fn test_validate_authorities_signatures() {
        let mut authority_signers = vec![];
        let secp = Secp256k1::new();
        let (secret_key1, public_key1) = secp.generate_keypair(&mut OsRng);
        let (secret_key2, public_key2) = secp.generate_keypair(&mut OsRng);
        authority_signers.push(public_key1);
        authority_signers.push(public_key2);

        let hello_world_hash = sha256::Hash::hash("Hello world!".as_bytes());
        let message = Message::from(hello_world_hash);
        let signature1 = secp.sign_ecdsa_recoverable(&message, &secret_key1);
        let signature2 = secp.sign_ecdsa_recoverable(&message, &secret_key2);

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            Some(vec![signature1, signature2]),
            Some(authority_signers.clone()),
            None,
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
            [0u8; 32],
        );

        header
            .validate_authorities_signatures(
                &hello_world_hash.as_byte_array().to_vec(),
                &authority_signers,
            )
            .unwrap();

        // Should fail if there is a signature missing
        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            Some(vec![signature1]),
            Some(authority_signers.clone()),
            None,
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
            [0u8; 32],
        );

        assert!(header
            .validate_authorities_signatures(
                &hello_world_hash.as_byte_array().to_vec(),
                &authority_signers,
            )
            .is_err());

        // Should fail if there is a signature from non-authority
        let (secret_key3, _public_key3) = secp.generate_keypair(&mut OsRng);
        let signature3 = secp.sign_ecdsa_recoverable(&message, &secret_key3);
        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            Some(vec![signature1, signature3]),
            Some(authority_signers.clone()),
            None,
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
            [0u8; 32],
        );

        assert!(header
            .validate_authorities_signatures(
                &hello_world_hash.as_byte_array().to_vec(),
                &authority_signers,
            )
            .is_err());
    }

    #[test]
    fn can_recover_authority_after_serialize() {
        let mut authority_signers = vec![];
        let secp = Secp256k1::new();
        let (secret_key, public_key) = secp.generate_keypair(&mut OsRng);
        authority_signers.push(public_key);

        let hello_world_hash = sha256::Hash::hash("Hello world!".as_bytes());
        let message = Message::from(hello_world_hash);
        let signature = secp.sign_ecdsa_recoverable(&message, &secret_key);

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            Some(vec![signature]),
            Some(authority_signers.clone()),
            None,
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
            [0u8; 32],
        );

        let serialized = header.serialize();

        let deserialized_header = ExtraDataHeader::deserialize(&mut serialized.as_slice())
            .expect("Deserialization failed");

        let recovered_pk =
            deserialized_header.authority_signatures.unwrap()[0].recover(&message).unwrap();

        assert_eq!(recovered_pk, public_key);
    }

    // Test case for validating with an invalid signature
    #[test]
    fn test_validate_authorities_signature_with_invalid_signatures() {
        let mut authority_signers = vec![];
        let secp = Secp256k1::new();
        let (secret_key, public_key) = secp.generate_keypair(&mut OsRng);
        authority_signers.push(public_key);

        let hello_world_hash = sha256::Hash::hash("Hello world!".as_bytes());
        let message = Message::from(hello_world_hash);
        let signature = secp.sign_ecdsa_recoverable(&message, &secret_key);

        let header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            Some(vec![signature]),
            Some(authority_signers.clone()),
            None,
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
            [0u8; 32],
        );
        let invalid_hash = sha256::Hash::hash("Not hello world!".as_bytes());
        let result = header.validate_authorities_signatures(
            &invalid_hash.as_byte_array().to_vec(),
            &authority_signers,
        );
        assert_eq!(result.unwrap_err(), ValidateAuthoritySignatureError::InvalidSignature)
    }

    // Test case for validating without a signature
    #[test]
    fn test_validate_authorities_signature_without_signatures() {
        let mut authority_signers = vec![];
        let secp = Secp256k1::new();
        let (_, public_key) = secp.generate_keypair(&mut OsRng);
        authority_signers.push(public_key);

        let header_without_signature = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            None,
            Some(authority_signers.clone()),
            None,
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
            [0u8; 32],
        );

        let message = vec![0u8; 32];

        let result =
            header_without_signature.validate_authorities_signatures(&message, &authority_signers);
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
            Some(vec![signature]),
            Some(authority_signers),
            None,
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
            [0u8; 32],
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
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
            [0u8; 32],
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
        assert_eq!(deserialized_header.authority_signatures, None);
    }

    #[test]
    fn can_set_signature() {
        let mut edh = ExtraDataHeader::default();

        assert_eq!(edh.authority_signatures, None);
        assert_eq!(edh.optional_fields, 0);
        let secp = Secp256k1::new();
        let (secret_key, _public_key) = secp.generate_keypair(&mut OsRng);

        let hello_world_hash = sha256::Hash::hash("Hello world!".as_bytes());
        let message = Message::from(hello_world_hash);
        let signature = secp.sign_ecdsa_recoverable(&message, &secret_key);

        edh.set_signature(vec![signature]);
        assert_eq!(edh.authority_signatures.is_some(), true);
        assert_eq!(edh.optional_fields, 1 << HAS_SIGNATURE_POS);
    }

    #[test]
    fn can_add_individual_signature() {
        let mut edh = ExtraDataHeader::default();
        let secp = Secp256k1::new();
        let (secret_key, _public_key) = secp.generate_keypair(&mut OsRng);

        let hello_world_hash = sha256::Hash::hash("foo bar".as_bytes());
        let message = Message::from(hello_world_hash);
        let signature = secp.sign_ecdsa_recoverable(&message, &secret_key);

        edh.add_signature(signature);
        assert_eq!(edh.authority_signatures.is_some(), true);
        let edh_signature = edh.authority_signatures.clone().unwrap();
        assert_eq!(edh_signature.clone().len(), 1);
        // make sure its the same signature
        assert_eq!(
            edh_signature.get(0).expect("valid sig").serialize_compact().1,
            signature.serialize_compact().1
        );

        assert_eq!(edh.optional_fields, 1 << HAS_SIGNATURE_POS);

        // can't add the same signature twice
        let mut edh2 = edh.clone();
        edh2.add_signature(signature);
        let edh_signature = edh2.authority_signatures.unwrap();
        assert_eq!(edh_signature.len(), 1);

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
            None,
            bitcoin::hash_types::BlockHash::all_zeros(),
            [0u8; 32],
        );

        println!("serialized header: {}", hex::encode(extra_data_header.serialize()));
    }
}
