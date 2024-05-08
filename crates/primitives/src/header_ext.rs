use crate::extra_data_header::{ExtraDataHeader, ExtraDataHeaderDeserializeError};
use crate::{Bytes, Header, B256};
use thiserror::Error;

/// Extension trait for the block header
/// Mainly adding extra data header utility functions
pub trait HeaderExt {
    /// serilaizes and adds extra data header to the header
    fn add_extra_data_header(&mut self, edh: &ExtraDataHeader);

    /// Attempts to deserialize the extra data header from the header
    fn deserialize_extra_data_header(
        &self,
    ) -> Result<ExtraDataHeader, ExtraDataHeaderDeserializeError>;

    /// Create signable sighash from header + edh content
    fn create_sighash(&self) -> Result<B256, ExtraDataHeaderDeserializeError>;

    /// Sign a block and update edh
    fn sign_block(
        &mut self,
        sk: &secp256k1::SecretKey,
    ) -> Result<(), ExtraDataHeaderDeserializeError>;

    /// Get the authority list from the extra data header
    fn get_authority_list(&self) -> Result<Option<Vec<secp256k1::PublicKey>>, GetAuthoritiesError>;

    /// Recover the signed authorities from the extra data header
    fn recovered_signed_authorities(
        &self,
    ) -> Result<Vec<secp256k1::PublicKey>, RecoverAuthorityError>;

    /// Validate the authority that produced the block was in turn according to the block timestamp
    fn validate_inturn(
        &self,
        authorities: &[secp256k1::PublicKey],
    ) -> Result<(), ValidateInturnError>;

    /// Get the block hash excluding the authority signatures
    fn segregated_signature_block_hash(&self) -> Result<B256, ExtraDataHeaderDeserializeError>;
}

#[derive(Debug, Error)]
/// Error that can occur while recovering the authority list
pub enum RecoverAuthorityError {
    #[error("No signature present in the extra data")]
    /// Signature is missing in the extra data
    NoSignaturePresentInExtraData,
    #[error("Failed to recover signer via ecdsa signature: {0}")]
    /// ecdsa Signature was not recoverable
    FailedToRecoverSigner(secp256k1::Error),
    #[error("Failed to deserialize the extra data: {0}")]
    /// Failed to deserialize the extra data
    FailedToDerserializeExtraData(#[from] ExtraDataHeaderDeserializeError),
}

#[derive(Debug, Error)]
/// Errors that can occur while reading the authority list from the block header
pub enum GetAuthoritiesError {
    #[error("Failed to deserialize the extra data: {0}")]
    /// Failed to deserialize the extra data
    FailedToRecoverAuthorityList(#[from] ExtraDataHeaderDeserializeError),
    /// Failed to retrive authorities, most likely this is not a epoch block
    #[error("Failed to retrieve authority list")]
    FailedToRetrieveAuthorityList,
    #[error("Failed to find authority index")]
    /// Failed to find authority index
    FailedToFindAuthoritySignerIndex,
    #[error("Failed to find epoch block")]
    /// Could not find any epoch blocks
    FailedToRetrieveEpochHeader,
}

#[derive(Debug, Error)]
/// Valid in turn error
pub enum ValidateInturnError {
    #[error("Authority not in turn")]
    /// Authority not in turn
    AuthorityNotInTurn,
    #[error("Failed to recover signer via ecdsa signature: {0}")]
    /// ecdsa Signature was not recoverable
    FailedToRecoverSigner(#[from] RecoverAuthorityError),
}

impl HeaderExt for Header {
    /// Adds extra data header to the header
    fn add_extra_data_header(&mut self, edh: &ExtraDataHeader) {
        self.extra_data = Bytes::from(edh.serialize());
    }

    /// Provides block hash without extra data header bytes
    fn segregated_signature_block_hash(&self) -> Result<B256, ExtraDataHeaderDeserializeError> {
        let mut this = self.clone();
        let mut edh = this.deserialize_extra_data_header()?;
        edh.authority_signatures = None;
        edh.set_optional_fields_bitmask();

        let mut writer: Vec<u8> = vec![];
        edh.encode_into_without_signature(&mut writer).expect("Valid extra data header");
        // Take ownership of the data in writer and leave an empty Vec<u8>
        let bytes_data = Bytes::from(writer.clone());
        this.extra_data = bytes_data;
        let hash = this.hash_slow();

        Ok(hash)
    }

    /// Validates that the authority in the first signature position was in turn when producing the block
    fn validate_inturn(
        &self,
        authorities: &[secp256k1::PublicKey],
    ) -> Result<(), ValidateInturnError> {
        let signers = self.recovered_signed_authorities()?;
        let in_turn_signer = signers.get(0).expect("at least one signer");
        let signer_index = authorities
            .iter()
            .position(|pk| pk == in_turn_signer)
            .ok_or(ValidateInturnError::AuthorityNotInTurn)?;

        let authorities_len = authorities.len() as u64;
        let block_timestamp_min = self.timestamp / 60;
        if (block_timestamp_min / authorities_len) % authorities_len != (signer_index as u64) {
            return Err(ValidateInturnError::AuthorityNotInTurn);
        }

        Ok(())
    }

    /// Recover the signed authorities from the extra data header
    fn recovered_signed_authorities(
        &self,
    ) -> Result<Vec<secp256k1::PublicKey>, RecoverAuthorityError> {
        let sighash = self.create_sighash()?;
        let message = secp256k1::Message::from_slice(sighash.as_slice())
            .expect("Valid message to recover signers");
        let edh = self.deserialize_extra_data_header()?;

        if let Some(signatures) = edh.authority_signatures {
            let signers = signatures
                .iter()
                .map(|sig| {
                    sig.recover(&message).map_err(RecoverAuthorityError::FailedToRecoverSigner)
                })
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(signers);
        }

        Err(RecoverAuthorityError::NoSignaturePresentInExtraData)
    }

    /// Get the authority list from the extra data header. If one exists
    fn get_authority_list(&self) -> Result<Option<Vec<secp256k1::PublicKey>>, GetAuthoritiesError> {
        let signers = self.deserialize_extra_data_header()?.authority_signers;

        Ok(signers)
    }

    /// deserialize the extra data header from the header
    fn deserialize_extra_data_header(
        &self,
    ) -> Result<ExtraDataHeader, ExtraDataHeaderDeserializeError> {
        let binding = self.extra_data.to_vec();
        let mut extra_data = binding.as_slice();
        Ok(ExtraDataHeader::deserialize(&mut extra_data)?)
    }

    /// Create signable sighash from header + edh content
    fn create_sighash(&self) -> Result<B256, ExtraDataHeaderDeserializeError> {
        let mut this = self.clone();
        let mut edh = this.deserialize_extra_data_header()?;
        edh.authority_signatures = None;
        edh.set_optional_fields_bitmask();

        let mut writer: Vec<u8> = vec![];
        edh.encode_into_without_signature(&mut writer).expect("Valid extra data header");
        // Take ownership of the data in writer and leave an empty Vec<u8>
        let bytes_data = Bytes::from(writer.clone());
        this.extra_data = bytes_data;
        let hash = this.hash_slow();

        Ok(hash)
    }

    /// Sign a block and update edh
    fn sign_block(
        &mut self,
        sk: &secp256k1::SecretKey,
    ) -> Result<(), ExtraDataHeaderDeserializeError> {
        let sighash = self.create_sighash()?;
        let message =
            secp256k1::Message::from_slice(sighash.as_slice()).expect("Valid message to sign");
        let signature = secp256k1::SECP256K1.sign_ecdsa_recoverable(&message, &sk);

        let mut edh = self.deserialize_extra_data_header()?;
        edh.add_signature(signature);

        self.extra_data = Bytes::from(edh.serialize());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bytes;
    use crate::Header;
    use std::str::FromStr;

    #[allow(dead_code)]
    const EDH_DEFAULT_SIGHASH: &str =
        "0xaaa3492fe3eec8da1ca35aca5930a44b1a5805e813bdd1773678b5041d905276";

    #[allow(dead_code)]
    const SK1: &str = "1aabc5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019";
    #[allow(dead_code)]
    const SK2: &str = "1bc1f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019";

    #[allow(dead_code)]
    fn generate_secret_key(hex_string: &str) -> secp256k1::SecretKey {
        secp256k1::SecretKey::from_str(hex_string).expect("Invalid hex string for SecretKey")
    }
    #[allow(dead_code)]
    fn sign_block_helper(header: &mut Header, signer_sk: Option<&str>) {
        let mut edh = ExtraDataHeader::default();
        let sk1 = generate_secret_key(SK1);
        let sk2 = generate_secret_key(SK2);

        edh.authority_signers = Some(vec![
            secp256k1::PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), &sk1),
            secp256k1::PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), &sk2),
        ]);
        edh.set_optional_fields_bitmask();
        header.extra_data = Bytes::from(edh.serialize());

        header.number = 1;
        if let Some(sk_str) = signer_sk {
            let sk = generate_secret_key(sk_str);
            header.sign_block(&sk).unwrap();
        } else {
            header.sign_block(&sk1).unwrap();
        }
    }

    #[test]
    fn block_hash_shouldnt_change_after_adding_signatures() {
        let mut header = Header::default();
        let mut edh = ExtraDataHeader::default();
        let sk1 = generate_secret_key(SK1);

        edh.authority_signers = Some(vec![
            secp256k1::PublicKey::from_secret_key(
                &secp256k1::Secp256k1::new(),
                &secp256k1::SecretKey::from_str(SK1).unwrap(),
            ),
            secp256k1::PublicKey::from_secret_key(
                &secp256k1::Secp256k1::new(),
                &secp256k1::SecretKey::from_str(SK2).unwrap(),
            ),
        ]);
        edh.set_optional_fields_bitmask();
        header.extra_data = Bytes::from(edh.serialize());
        let hash_before = header.segregated_signature_block_hash().expect("valid hash");

        header.sign_block(&sk1).unwrap();
        let hash_after = header.segregated_signature_block_hash().expect("valid hash");

        assert_eq!(hash_before, hash_after);
    }

    #[test]
    fn create_default_edh_sighhash() {
        let edh = ExtraDataHeader::default();
        let mut header = Header::default();
        header.extra_data = Bytes::from(edh.serialize());
        let sighash = header.create_sighash().unwrap();

        assert_eq!(sighash.to_string(), EDH_DEFAULT_SIGHASH);
    }

    #[test]
    fn create_sighash_with_authority_signature() {
        // regardless of the signature, the sighash should be the same
        // This is because we remove the signature from the extra data header before signing
        let mut edh = ExtraDataHeader::default();
        edh.add_signature(
            secp256k1::ecdsa::RecoverableSignature::from_compact(
                &[0u8; 64],
                secp256k1::ecdsa::RecoveryId::from_i32(1i32).unwrap(),
            )
            .unwrap(),
        );
        let mut header = Header::default();
        header.extra_data = Bytes::from(edh.serialize());
        let sighash = header.create_sighash().unwrap();

        assert_eq!(sighash.to_string(), EDH_DEFAULT_SIGHASH);
    }

    #[test]
    fn create_sighash_with_authorities() {
        // However adding something else such as authority members should result in a different
        // sighash
        let mut edh = ExtraDataHeader::default();
        edh.authority_signers = Some(vec![
            secp256k1::PublicKey::from_secret_key(
                &secp256k1::Secp256k1::new(),
                &secp256k1::SecretKey::from_str(
                    "1aabc5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019",
                )
                .unwrap(),
            ),
            secp256k1::PublicKey::from_secret_key(
                &secp256k1::Secp256k1::new(),
                &secp256k1::SecretKey::from_str(
                    "1bc1f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019",
                )
                .unwrap(),
            ),
        ]);
        edh.set_optional_fields_bitmask();
        let mut header = Header::default();
        header.extra_data = Bytes::from(edh.serialize());

        let sighash = header.create_sighash().unwrap();
        assert_ne!(sighash.to_string(), EDH_DEFAULT_SIGHASH);
    }

    #[test]
    fn should_recover_authorities() {
        // However adding something else such as authority members should result in a different
        // sighash
        let mut edh = ExtraDataHeader::default();
        edh.authority_signers = Some(vec![
            secp256k1::PublicKey::from_secret_key(
                &secp256k1::Secp256k1::new(),
                &secp256k1::SecretKey::from_str(
                    "1aabc5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019",
                )
                .unwrap(),
            ),
            secp256k1::PublicKey::from_secret_key(
                &secp256k1::Secp256k1::new(),
                &secp256k1::SecretKey::from_str(
                    "1bc1f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe058f094135f2c9b019",
                )
                .unwrap(),
            ),
        ]);
        edh.set_optional_fields_bitmask();
        let mut header = Header::default();
        header.extra_data = Bytes::from(edh.serialize());

        let auths = header.get_authority_list().unwrap();
        assert!(auths.is_some());
        assert_eq!(auths.unwrap(), edh.authority_signers.unwrap());
    }

    // Get authority list tests
    #[test]
    fn should_recover_none_authorities() {
        let edh = ExtraDataHeader::default();
        let mut header = Header::default();
        header.extra_data = Bytes::from(edh.serialize());
        let signer_list = header.get_authority_list().unwrap();

        assert!(signer_list.is_none());
    }

    #[test]
    fn deserialize_extension_trait() {
        let mut header = Header::default();
        let edh = ExtraDataHeader::default();
        let serialized = edh.serialize();
        header.extra_data = serialized.into();
        let deserialized_edh =
            header.deserialize_extra_data_header().expect("Deserialization passed");

        assert_eq!(deserialized_edh, edh);
    }

    #[test]
    fn fails_to_recover_when_edh_invalid() {
        let mut header = Header::default();
        header.extra_data = Bytes::from("foobar");
        let signer_list = header.get_authority_list();

        assert!(signer_list.is_err());
    }

    #[test]
    fn should_recover_signed_authority() {
        let mut header = Header::default();
        let mut edh = ExtraDataHeader::default();
        let sk1 = generate_secret_key(SK1);
        let sk2 = generate_secret_key(SK2);

        edh.authority_signers = Some(vec![
            secp256k1::PublicKey::from_secret_key(
                &secp256k1::Secp256k1::new(),
                &secp256k1::SecretKey::from_str(SK1).unwrap(),
            ),
            secp256k1::PublicKey::from_secret_key(
                &secp256k1::Secp256k1::new(),
                &secp256k1::SecretKey::from_str(SK2).unwrap(),
            ),
        ]);
        edh.set_optional_fields_bitmask();
        let mut header = Header::default();
        header.extra_data = Bytes::from(edh.serialize());
        header.sign_block(&sk1).unwrap();

        let recovered = header.recovered_signed_authorities().unwrap();
        // utility function above only signs with the first authority
        assert_eq!(recovered[0], edh.clone().authority_signers.unwrap()[0]);

        // Now sign with the second authority
        header.sign_block(&sk2).unwrap();
        let recovered = header.recovered_signed_authorities().unwrap();
        assert_eq!(recovered[0], edh.clone().authority_signers.unwrap()[0]);
        assert_eq!(recovered[1], edh.clone().authority_signers.unwrap()[1]);
    }

    #[test]
    fn validate_inturn_ok() {
        let sk1 = generate_secret_key(SK1);
        let sk2 = generate_secret_key(SK2);
        let pks = [sk1.public_key(secp256k1::SECP256K1), sk2.public_key(secp256k1::SECP256K1)];
        let mut header = Header::default();
        header.timestamp = 1705621229;
        sign_block_helper(&mut header, Some(SK1));
        assert!(header.validate_inturn(&pks).is_ok());
        // Sign the same header with a different key should fail
        sign_block_helper(&mut header, Some(SK2));
        assert!(header.validate_inturn(&pks).is_err());
    }
}
