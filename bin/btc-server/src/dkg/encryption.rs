use super::{Initiator, Target};
use bitcoin::secp256k1;
use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit, Nonce};
use frost::keys::dkg::{round1, round2};
use frost_secp256k1_tr as frost;
use merlin::Transcript;
use std::{
    collections::{BTreeMap, BTreeSet},
    vec,
};
use thiserror::Error;
use zeroize::Zeroizing;

/// Entry for managing symmetric keys used in round two encryption.
///
/// Contains separate keys for sending and receiving messages to/from a specific
/// participant, along with a nonce counter for outgoing messages.
#[derive(Clone)]
struct SymmetricKeyEntry {
    sending: Zeroizing<[u8; 32]>,
    receiving: Zeroizing<[u8; 32]>,
    // Our sending nonce; this is incremented for each message we send. The
    // receiving nonce is specified in the incoming message.
    nonce: u64,
}

/// Converts a u64 nonce to a 12-byte serialized nonce for use with the
/// ChaCha20Poly1305 encryption algorithm.
fn integer_to_serialized_nonce(nonce: u64) -> Nonce {
    let mut nonce_bytes = [0; 12];
    nonce_bytes[..8].copy_from_slice(&nonce.to_le_bytes());
    *Nonce::from_slice(&nonce_bytes)
}

/// Possible errors that can occur during the DKG authentication process.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// The initialized federation does not contain the participant's FROST ID.
    #[error("My Frost ID is not present in the federation")]
    SelfNotInFederation,
    /// Not enough participants have submitted their packages to proceed
    #[error("Insufficient samples to proceed to the next stage")]
    InsufficientSamples,
    /// The specified participant is not a member of the federation
    #[error("Not a member of the federation")]
    NotAFedMember,
    /// A signature verification has failed
    #[error("Signature verification failed")]
    SignatureVerificationFailed,
    /// Failed to decrypt a message, possibly due to tampering or incorrect keys
    #[error("Decryption failed")]
    DecryptionFailed,
    /// Failed to deserialize a decrypted message
    #[error("Deserialization failed")]
    DeserializationFailed,
    /// We need to generate our own round3 commitment before processing others
    #[error("Awaiting our round3 commitment to generate the challenge")]
    AwaitingChallengeGeneration,
    /// An unexpected internal error from the FROST library
    #[allow(clippy::enum_variant_names)]
    #[error("Unexpected internal Frost error")]
    InternalFrostError(#[from] frost::Error),
    /// An unexpected internal error from the encryption library
    #[allow(clippy::enum_variant_names)]
    #[error("Unexpected internal Aead error")]
    InternalAeadError(chacha20poly1305::aead::Error),
}

impl From<chacha20poly1305::aead::Error> for Error {
    fn from(err: chacha20poly1305::aead::Error) -> Self {
        Error::InternalAeadError(err)
    }
}

/// Authentication and encryption layer for round one of the DKG protocol.
///
/// Handles the authentication of round one packages using ephemeral keys and
/// signatures, establishing a secure communication channel between
/// participants.
#[derive(Clone)]
pub struct DkgHandshakeManager {
    secp: secp256k1::Secp256k1<secp256k1::All>,
    transcript: Transcript,
    my_frost_id: frost::Identifier,
    my_static_sec: secp256k1::SecretKey,
    my_eph_sec: secp256k1::SecretKey,
    my_eph_pub: secp256k1::PublicKey,
    fed_members: BTreeMap<frost::Identifier, secp256k1::PublicKey>,
    eph_keys: BTreeMap<frost::Identifier, secp256k1::PublicKey>,
    round1_commits: BTreeMap<frost::Identifier, Vec<u8>>,
}

impl std::fmt::Debug for DkgHandshakeManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DkgHandshakeManager")
            .field("transcript", &"[REDACTED]")
            .field("my_frost_id", &self.my_frost_id)
            .field("my_static_sec", &"[REDACTED]")
            .field("my_eph_sec", &"[REDACTED]")
            .field("my_eph_pub", &self.my_eph_pub)
            .field("fed_members", &self.fed_members)
            .field("eph_keys", &self.eph_keys)
            .field("round1_commits", &self.round1_commits)
            .finish()
    }
}

impl DkgHandshakeManager {
    /// Creates the first instance for starting a new DKG session.
    ///
    /// # Arguments
    ///
    /// * `context` - Unique identifier for this DKG session
    /// * `nonce` - Unique nonce for this DKG session
    /// * `my_frost_id` - The FROST identifier of this participant
    /// * `my_static_sec` - The static secret key of this participant
    /// * `fed_members` - Map of all federation members' FROST IDs to their public keys
    pub fn new(
        context: &[u8],
        nonce: u64,
        my_frost_id: frost::Identifier,
        my_static_sec: secp256k1::SecretKey,
        fed_members: BTreeMap<frost::Identifier, secp256k1::PublicKey>,
    ) -> Result<Self, Error> {
        if !fed_members.contains_key(&my_frost_id) {
            return Err(Error::SelfNotInFederation);
        }

        let mut t = Transcript::new(b"Botanix_Macbeth_DKG_Setup_v1");
        t.append_message(b"context", context);
        t.append_u64(b"nonce", nonce);

        let secp = secp256k1::Secp256k1::new();
        let my_eph_sec = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let my_eph_pub = secp256k1::PublicKey::from_secret_key(&secp, &my_eph_sec);

        // Track my eph key
        let mut eph_keys = BTreeMap::new();
        eph_keys.insert(my_frost_id, my_eph_pub);

        Ok(DkgHandshakeManager {
            secp,
            transcript: t,
            my_frost_id,
            my_static_sec,
            my_eph_sec,
            my_eph_pub,
            fed_members,
            eph_keys,
            round1_commits: BTreeMap::new(),
        })
    }
    /// Signs a round one package for authentication.
    ///
    /// Creates a commitment to the round one package, signs it with the
    /// participant's static secret key, and returns the ephemeral public key
    /// and signature.
    ///
    /// # Arguments
    ///
    /// * `package` - The round one package to be committed
    ///
    /// # Returns
    ///
    /// A tuple containing the ephemeral public key and signature for the
    /// package
    pub fn commit_round1(
        &mut self,
        package: &round1::Package,
    ) -> Result<(secp256k1::PublicKey, secp256k1::ecdsa::Signature), Error> {
        let mut commit = vec![0; 32];
        let my_eph_pub = self.my_eph_pub;

        // Compute the challenge bytes to be signed; notably, we commit the
        // ephemeral key and the round1 package.
        let mut t = self.transcript.clone();
        t.append_message(b"frost_id", self.my_frost_id.serialize().as_slice());
        t.append_message(b"eph_pub", my_eph_pub.serialize().as_slice());
        t.append_message(b"round1_package", package.serialize()?.as_slice());
        t.challenge_bytes(b"round1_commit", &mut commit);
        std::mem::drop(t);

        // Create the signature
        let msg = secp256k1::Message::from_digest_slice(&commit).expect("valid size");
        let signature = self.secp.sign_ecdsa(&msg, &self.my_static_sec);

        // Keep track of our commitment bytes as well.
        self.round1_commits.insert(self.my_frost_id, commit);

        Ok((my_eph_pub, signature))
    }
    /// Validates the authenticity of a received round one package.
    ///
    /// Verifies the signature on the round one package using the sender's
    /// public key and stores the ephemeral key for later use.
    ///
    /// # Arguments
    ///
    /// * `initiator` - The FROST identifier of the package initiator
    /// * `eph_pub` - The ephemeral public key of the initiator
    /// * `signature` - The signature to verify
    /// * `package` - The round one package being authenticated
    ///
    /// # Returns
    ///
    /// Ok(()) if validation succeeds, Error otherwise
    pub fn validate_round1(
        &mut self,
        initiator: Initiator,
        eph_pub: secp256k1::PublicKey,
        signature: secp256k1::ecdsa::Signature,
        package: &round1::Package,
    ) -> Result<(), Error> {
        let mut commit = vec![0; 32];
        let fed_static = self.fed_members.get(&initiator.0).ok_or(Error::NotAFedMember)?;

        // Compute the challenge bytes to be verified against the provided
        // signature; notably, we commit the ephemeral key and the round1
        // package.
        let mut t = self.transcript.clone();
        t.append_message(b"frost_id", initiator.0.serialize().as_slice());
        t.append_message(b"eph_pub", eph_pub.serialize().as_slice());
        t.append_message(b"round1_package", package.serialize()?.as_slice());
        t.challenge_bytes(b"round1_commit", &mut commit);
        std::mem::drop(t);

        // Verify the signature using the public key of the fed member.
        let msg = secp256k1::Message::from_digest_slice(&commit).expect("valid size");

        if fed_static.verify(&self.secp, &msg, &signature).is_err() {
            return Err(Error::SignatureVerificationFailed);
        }

        // Keep track of the ephemeral key.
        self.eph_keys.insert(initiator.0, eph_pub);

        // Keep track of the challenge bytes.
        self.round1_commits.insert(initiator.0, commit);

        Ok(())
    }
    /// Finalizes round one and transitions to round two.
    ///
    /// Processes all collected round one commitments, establishes symmetric
    /// keys for encrypted communication in round two, and creates a
    /// `SecureChannelManager` instance.
    ///
    /// # Returns
    ///
    /// A SecureChannelManager instance if all required commitments are collected, Error
    /// otherwise
    pub fn finalize(&mut self) -> Result<SecureChannelManager, Error> {
        // Validate basic conditions.
        if self.round1_commits.len() != self.fed_members.len() {
            return Err(Error::InsufficientSamples);
        }

        debug_assert_eq!(self.round1_commits.len(), self.fed_members.len());
        debug_assert_eq!(self.eph_keys.len(), self.fed_members.len());

        let mut round1_commits: Vec<(frost::Identifier, Vec<u8>)> =
            std::mem::take(&mut self.round1_commits).into_iter().collect();

        // Sort in ascending order, by the frost_id.
        round1_commits.sort_by(|a, b| a.0.cmp(&b.0));

        // Append all the commitments to the transcript.
        for (_, commit) in round1_commits {
            self.transcript.append_message(b"round1_commit", &commit);
        }

        let mut symmetric_keys = BTreeMap::new();

        // Prepare all symmetric keys for the next stage.
        for (fed_id, fed_static) in &self.fed_members {
            if fed_id == &self.my_frost_id {
                continue;
            }

            let fed_eph = self.eph_keys.get(fed_id).expect("ephemeral key must exist");

            // Compute the shared secrets; the participant with the lower Frost
            // ID generates the following order:
            //
            // * ss1 = DH(my_static_key, their_ephemeral_key)
            // * ss2 = DH(their_static_key, my_ephemeral_key)
            //
            // The other participant generates this in the opposite direction,
            // of course.
            let ss1: secp256k1::ecdh::SharedSecret;
            let ss2: secp256k1::ecdh::SharedSecret;

            debug_assert_ne!(&self.my_frost_id, fed_id);
            if &self.my_frost_id < fed_id {
                ss1 = secp256k1::ecdh::SharedSecret::new(fed_eph, &self.my_static_sec);
                ss2 = secp256k1::ecdh::SharedSecret::new(fed_static, &self.my_eph_sec);
            } else {
                ss1 = secp256k1::ecdh::SharedSecret::new(fed_static, &self.my_eph_sec);
                ss2 = secp256k1::ecdh::SharedSecret::new(fed_eph, &self.my_static_sec);
            }

            let mut key1 = Zeroizing::new([0; 32]);
            let mut key2 = Zeroizing::new([0; 32]);

            // Append the shared secrets to the transcript and enerate the
            // symmetric keys - reproducible KDFs for both participants.
            let mut t = self.transcript.clone();
            t.append_message(b"ss1", ss1.secret_bytes().as_slice());
            t.append_message(b"ss2", ss2.secret_bytes().as_slice());
            t.challenge_bytes(b"sym_key1", key1.as_mut_slice());
            t.challenge_bytes(b"sym_key2", key2.as_mut_slice());
            std::mem::drop(t);

            // The participant with the lower Frost ID uses the first key as the
            // sending key and the second key as the receiving key. The other
            // participant does the opposite.
            debug_assert_ne!(&self.my_frost_id, fed_id);
            let (sending, receiving) =
                if &self.my_frost_id < fed_id { (key1, key2) } else { (key2, key1) };

            // Track the symmetric key for this target.
            let entry = SymmetricKeyEntry { sending, receiving, nonce: 0 };

            symmetric_keys.insert(*fed_id, entry);
        }

        Ok(SecureChannelManager {
            secp: self.secp.clone(),
            transcript: self.transcript.clone(),
            my_frost_id: self.my_frost_id,
            my_static_sec: self.my_static_sec,
            fed_members: std::mem::take(&mut self.fed_members),
            symmetric_keys,
            round2_checks: BTreeSet::new(),
        })
    }
}

/// Authentication and encryption layer for round two of the DKG protocol.
///
/// Handles the encryption and decryption of round two packages using symmetric
/// keys established during round one.
#[derive(Clone)]
pub struct SecureChannelManager {
    secp: secp256k1::Secp256k1<secp256k1::All>,
    transcript: Transcript,
    my_frost_id: frost::Identifier,
    my_static_sec: secp256k1::SecretKey,
    fed_members: BTreeMap<frost::Identifier, secp256k1::PublicKey>,
    symmetric_keys: BTreeMap<frost::Identifier, SymmetricKeyEntry>,
    round2_checks: BTreeSet<frost::Identifier>,
}

impl std::fmt::Debug for SecureChannelManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecureChannelManager")
            .field("transcript", &"[REDACTED]")
            .field("my_frost_id", &self.my_frost_id)
            .field("my_static_sec", &"[REDACTED]")
            .field("fed_members", &self.fed_members)
            .field("symmetric_keys", &"[REDACTED]")
            .field("round2_checks", &self.round2_checks)
            .finish()
    }
}

impl SecureChannelManager {
    /// Encrypts a round two package for a specific target.
    ///
    /// # Arguments
    ///
    /// * `target` - The FROST identifier of the intended recipient
    /// * `package` - The round two package to encrypt
    ///
    /// # Returns
    ///
    /// A tuple containing the encrypted package and the nonce used for
    /// encryption
    pub fn commit_round2(
        &mut self,
        target: &Target,
        package: &round2::Package,
    ) -> Result<(Vec<u8>, u64), Error> {
        // Retrieve the symmetric key for the target.
        let entry = self.symmetric_keys.get_mut(&target.0).ok_or(Error::NotAFedMember)?;

        let sending_nonce = entry.nonce;
        let ser_nonce = integer_to_serialized_nonce(sending_nonce);

        // Increment nonce for the next (re-)send message.
        entry.nonce += 1;

        // Encrypt the package using the symmetric key.
        let plaintext = package.serialize()?;
        let key = entry.sending.as_slice();
        let cipher = ChaCha20Poly1305::new_from_slice(key).expect("valid size");
        let enc_package = cipher.encrypt(&ser_nonce, plaintext.as_slice())?;

        // Mark our own check.
        self.round2_checks.insert(self.my_frost_id);

        Ok((enc_package, sending_nonce))
    }
    /// Decrypts and validates a received round two package.
    ///
    /// # Arguments
    ///
    /// * `initiator` - The FROST identifier of the package initiator
    /// * `nonce` - The nonce used for encryption
    /// * `package` - The encrypted round two package
    ///
    /// # Returns
    ///
    /// The decrypted round two package if validation succeeds, Error otherwise
    pub fn validate_round2(
        &mut self,
        initiator: Initiator,
        nonce: u64,
        package: &[u8],
    ) -> Result<round2::Package, Error> {
        // Prepare the encryption key.
        let entry = self.symmetric_keys.get(&initiator.0).ok_or(Error::NotAFedMember)?;

        let receiving_key = entry.receiving.as_slice();
        let cipher = ChaCha20Poly1305::new_from_slice(receiving_key).expect("valid size");
        let ser_nonce = integer_to_serialized_nonce(nonce);

        // Decrypt the package using the symmetric key.
        let plaintext = cipher.decrypt(&ser_nonce, package).map_err(|_| Error::DecryptionFailed)?;

        let package = round2::Package::deserialize(plaintext.as_slice())
            .map_err(|_| Error::DeserializationFailed)?;

        // Mark initiator as checked.
        self.round2_checks.insert(initiator.0);

        Ok(package)
    }
    /// Finalizes round two and transitions to round three.
    ///
    /// Ensures all required round two packages have been processed and creates
    /// a KeyVerificationManager instance.
    ///
    /// # Returns
    ///
    /// A `KeyVerificationManager` instance if all required packages are processed, Error
    /// otherwise
    pub fn finalize(&mut self) -> Result<KeyVerificationManager, Error> {
        if self.round2_checks.len() != self.fed_members.len() {
            return Err(Error::InsufficientSamples);
        }

        // Mark stage2 as complete; we just commit an empty message.
        self.transcript.append_message(b"round2_commit", b"");

        Ok(KeyVerificationManager {
            secp: self.secp.clone(),
            transcript: self.transcript.clone(),
            my_frost_id: self.my_frost_id,
            my_static_sec: self.my_static_sec,
            fed_members: std::mem::take(&mut self.fed_members),
            challenge: None,
            round3_commits: BTreeMap::new(),
        })
    }
}

/// Authentication layer for round three of the DKG protocol.
///
/// Handles the verification of signatures on the final aggregated public key
/// package to ensure all participants have the same view of the generated key.
#[derive(Clone)]
pub struct KeyVerificationManager {
    secp: secp256k1::Secp256k1<secp256k1::All>,
    transcript: Transcript,
    my_frost_id: frost::Identifier,
    my_static_sec: secp256k1::SecretKey,
    fed_members: BTreeMap<frost::Identifier, secp256k1::PublicKey>,
    challenge: Option<[u8; 32]>,
    round3_commits: BTreeMap<frost::Identifier, secp256k1::ecdsa::Signature>,
}

impl std::fmt::Debug for KeyVerificationManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyVerificationManager")
            .field("transcript", &"[REDACTED]")
            .field("my_frost_id", &self.my_frost_id)
            .field("my_static_sec", &"[REDACTED]")
            .field("fed_members", &self.fed_members)
            .field("challenge", &self.challenge)
            .field("round3_commits", &self.round3_commits)
            .finish()
    }
}

impl KeyVerificationManager {
    /// Creates a signature commitment on the final public key package.
    ///
    /// # Arguments
    ///
    /// * `package` - The final aggregated public key package
    ///
    /// # Returns
    ///
    /// A signature on the public key package for verification by other
    /// participants
    pub fn commit_round3(
        &mut self,
        package: &frost::keys::PublicKeyPackage,
    ) -> Result<secp256k1::ecdsa::Signature, Error> {
        let mut commit = [0; 32];

        self.transcript.append_message(b"round3_package", package.serialize()?.as_slice());
        self.transcript.challenge_bytes(b"round3_commit", &mut commit);

        let msg = secp256k1::Message::from_digest_slice(&commit).expect("valid size");
        let signature = self.secp.sign_ecdsa(&msg, &self.my_static_sec);

        // Set the unified challenge.
        self.challenge = Some(commit);

        // Keep track of our own signature.
        self.round3_commits.insert(self.my_frost_id, signature);

        Ok(signature)
    }
    /// Validates a received signature on the final public key package.
    ///
    /// # Arguments
    ///
    /// * `initiator` - The FROST identifier of the signature creator
    /// * `signature` - The signature to verify
    ///
    /// # Returns
    ///
    /// `Ok(())` if validation succeeds, Error otherwise
    pub fn validate_round3(
        &mut self,
        initiator: Initiator,
        signature: secp256k1::ecdsa::Signature,
    ) -> Result<(), Error> {
        let Some(challenge) = &self.challenge else {
            // We do not process incoming round3 messages until we have sent our
            // own, since we require the aggregated key package to compute the
            // challenge to be signed.
            return Err(Error::AwaitingChallengeGeneration);
        };

        let fed_static = self.fed_members.get(&initiator.0).ok_or(Error::NotAFedMember)?;

        // Verify the signature using the public key of the fed member.
        let msg = secp256k1::Message::from_digest_slice(challenge).expect("valid size");

        if fed_static.verify(&self.secp, &msg, &signature).is_err() {
            return Err(Error::SignatureVerificationFailed);
        }

        self.round3_commits.insert(initiator.0, signature);

        Ok(())
    }
    /// Finalizes round three, completing the DKG protocol.
    ///
    /// Ensures all required signatures have been verified and
    /// generates a final commitment value.
    ///
    /// # Returns
    ///
    /// A 32-byte array representing the final commitment if all required
    /// signatures are verified, Error otherwise
    pub fn finalize(&mut self) -> Result<[u8; 32], Error> {
        // Validate basic conditions.
        if self.round3_commits.len() != self.fed_members.len() {
            return Err(Error::InsufficientSamples);
        }

        let mut round3_commits: Vec<(frost::Identifier, secp256k1::ecdsa::Signature)> =
            std::mem::take(&mut self.round3_commits).into_iter().collect();

        // Sort in ascending order, by the frost_id.
        round3_commits.sort_by(|a, b| a.0.cmp(&b.0));

        let t = &mut self.transcript;
        for (_, signature) in round3_commits {
            t.append_message(b"round3_commit", signature.serialize_compact().as_slice());
        }

        let mut final_commit = [0; 32];
        t.challenge_bytes(b"final_commit", &mut final_commit);

        Ok(final_commit)
    }
}
