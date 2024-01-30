use frost::SigningPackage;
use frost_secp256k1_tr as frost;
use rand::thread_rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use thiserror::Error;

/// Datastructure for storing key information relevant to a particular multiset
/// Specifically we need to keep track of the following:
/// key_package
/// public_key_package
/// our personal identifier
/// round1 packages (if DKG is occuring)
/// round2 packages (if DKG is occuring)
/// Any secret packages (either personal or group) should be calculated on the fly
/// and not stored in the database
#[derive(Serialize, Deserialize, Clone)]
pub struct FrostState {
    pub min_signers: u16,
    pub max_signers: u16,
    pub personal_identifier: frost::Identifier,
    /// Dkg fields
    /// Optional incase DKG is already completed and we have a key package
    personal_round_1: Option<frost::keys::dkg::round1::Package>,
    #[serde(skip)]
    personal_secret_package: Option<frost::keys::dkg::round1::SecretPackage>,
    round1_group_packages: BTreeMap<frost::Identifier, frost::keys::dkg::round1::Package>,
    round2_group_packages: BTreeMap<frost::Identifier, frost::keys::dkg::round2::Package>,
    #[serde(skip)]
    round2_secret_package: Option<frost::keys::dkg::round2::SecretPackage>,
    /// Signing Fields
    pub key_package: Option<frost::keys::KeyPackage>,
    public_key_package: Option<frost::keys::PublicKeyPackage>,
    #[serde(skip)]
    signer_nonces: Option<frost::round1::SigningNonces>,
    /// Only available if we are the cordinator
    /// And during a signing session sessions
    signing_commitmentments: BTreeMap<frost::Identifier, frost::round1::SigningCommitments>,
    signature_shares: BTreeMap<frost::Identifier, frost::round2::SignatureShare>,
}

#[derive(Debug, Error)]
pub enum DKGError {
    #[error("missing personal secret package")]
    MissingPersonalSecretPackage,
    #[error("missing round 2 secret package")]
    MissingRound2SecretPackage,
    #[error("missing key package")]
    MissingKeyPackage,
    #[error("intenal frost error")]
    Frost(#[from] frost::Error),
}

#[derive(Debug, Error)]
pub enum SigningError {
    #[error("missing key package")]
    MissingKeyPackage,
    #[error("public key package")]
    MissingPublicKeyPackage,
    #[error("missing signer nonces")]
    MissingSignerNonces,
    #[error("intenal frost error")]
    Frost(#[from] frost::Error),
}

#[derive(Debug, Error)]
pub enum CordinatorError {
    #[error("exceeding max signing commitments")]
    ExceedingMaxNonceCommitments,
    #[error("duplicate signing commitment")]
    DuplicateSigningCommitment,
    #[error("duplicate signature share")]
    DuplicateSignatureShare,
    #[error("intenal frost error")]
    Frost(#[from] frost::Error),
}

/// Represents a set of keys used in a FROST scheme.
///
/// This struct provides methods for generating and managing keys during the distributed key
/// generation process. It supports operations such as setting personal round 1 package, setting
/// group packages for round 1 and round 2, generating personal round 1 and round 2 packages, adding
/// participant packages for round 1 and round 2, and creating a public key package.
///
/// # Examples
///
/// ```
/// use frost::Identifier;
/// use std::collections::BTreeMap;
///
/// let personal_identifier = Identifier::new();
/// let mut keys = Keys::new(3, 5, personal_identifier);
///
/// keys.generate_personal_round1_package().unwrap();
///
/// let peer_identifier = Identifier::new();
/// let peer_round1_package = frost::keys::dkg::round1::Package::new();
/// keys.add_participant_round1(peer_identifier, peer_round1_package);
///
/// keys.generate_personal_round2_package().unwrap();
///
/// let peer_round2_package = frost::keys::dkg::round2::Package::new();
/// keys.add_participant_round2(peer_identifier, peer_round2_package);
///
/// keys.create_pubkey_package().unwrap();
/// ```

impl FrostState {
    pub fn new(
        min_signers: u16,
        max_signers: u16,
        personal_identifier: frost::Identifier,
    ) -> FrostState {
        FrostState {
            min_signers,
            max_signers,
            personal_identifier,
            personal_round_1: None,
            round1_group_packages: BTreeMap::new(),
            personal_secret_package: None,
            round2_group_packages: BTreeMap::new(),
            round2_secret_package: None,
            key_package: None,
            public_key_package: None,
            signing_commitmentments: BTreeMap::new(),
            signature_shares: BTreeMap::new(),
            signer_nonces: None,
        }
    }

    /// Sets the personal round 1 package.
    ///
    /// # Arguments
    ///
    /// * `round1` - The personal round 1 package.
    pub fn set_personal_round_1(&mut self, round1: frost::keys::dkg::round1::Package) {
        self.personal_round_1 = Some(round1);
    }

    /// Sets the round 1 group package for the specified identifier.
    ///
    /// # Arguments
    ///
    /// * `identifier` - The identifier of the group.
    /// * `round1` - The round 1 group package.
    pub fn set_round1_group_package(
        &mut self,
        identifier: frost::Identifier,
        round1: frost::keys::dkg::round1::Package,
    ) {
        self.round1_group_packages.insert(identifier, round1);
    }

    /// Sets the round 2 group package for the specified identifier.
    ///
    /// # Arguments
    ///
    /// * `identifier` - The identifier of the group.
    /// * `round2` - The round 2 group package.
    pub fn set_round2_group_package(
        &mut self,
        identifier: frost::Identifier,
        round2: frost::keys::dkg::round2::Package,
    ) {
        self.round2_group_packages.insert(identifier, round2);
    }

    /** Round 1 utils * */
    /// Generates the personal round 1 package.
    ///
    /// # Returns
    ///
    /// An `Ok` result if the personal round 1 package is generated successfully,
    /// Note: the secret package is not saved in the database nor should it leave the btc server
    /// or an `Err` result with a `frost::Error` if an error occurs.
    pub fn generate_personal_round1_package(
        &self,
    ) -> Result<
        (frost::keys::dkg::round1::SecretPackage, frost::keys::dkg::round1::Package),
        frost::Error,
    > {
        let rng = thread_rng();
        let round1_dkg = frost::keys::dkg::part1(
            self.personal_identifier,
            self.max_signers,
            self.min_signers,
            rng,
        )?;

        // self.personal_round_1 = Some(round1_personal_package.clone());
        // self.personal_secret_package = Some(secret_package);

        Ok(round1_dkg)
    }

    /// Adds a participant's round 1 package.
    ///
    /// # Arguments
    ///
    /// * `peer_identifier` - The identifier of the participant.
    /// * `peer_round1_package` - The round 1 package of the participant.
    pub fn add_participant_round1(
        &mut self,
        peer_identifier: frost::Identifier,
        peer_round1_package: frost::keys::dkg::round1::Package,
    ) {
        self.round1_group_packages.insert(peer_identifier, peer_round1_package);
    }

    /** Round 2 utils */

    // Generates the personal round 2 package.
    ///
    /// # Returns
    ///
    /// An `Ok` result with a `BTreeMap` of round 2 packages for each peer,
    /// or an `Err` result with a `DKGError` if an error occurs.
    ///
    /// Expects that a peronal secret pacakge is created
    /// and that all round 1 packages are recieved from peers
    /// Will return a round 2 package to be sent to each peer
    /// this package is a commitment specific to each peer
    pub fn generate_personal_round2_package(
        &mut self,
    ) -> Result<BTreeMap<frost::Identifier, frost::keys::dkg::round2::Package>, DKGError> {
        if let Some(personal_secret_pacakge) = &self.personal_secret_package {
            let (round2_secret_package, round2_packages) = frost::keys::dkg::part2(
                personal_secret_pacakge.clone(),
                &self.round1_group_packages,
            )?;
            self.round2_secret_package = Some(round2_secret_package);
            return Ok(round2_packages)
        } else {
            return Err(DKGError::MissingPersonalSecretPackage);
        }
    }

    /// Adds a participant's round 2 package.
    ///
    /// # Arguments
    ///
    /// * `peer_identifier` - The identifier of the participant.
    /// * `peer_round2_package` - The round 2 package of the participant.
    pub fn add_participant_round2(
        &mut self,
        peer_identifier: frost::Identifier,
        peer_round2_package: frost::keys::dkg::round2::Package,
    ) {
        self.round2_group_packages.insert(peer_identifier, peer_round2_package);
    }

    /** Round 3 Utils * */
    /// Creates the public key package.
    ///
    /// # Returns
    ///
    /// An `Ok` result if the public key package is created successfully,
    /// or an `Err` result with a `DKGError` if an error occurs.
    pub fn create_pubkey_package(&mut self) -> Result<(), DKGError> {
        if let Some(round2_secret_package) = &self.round2_secret_package {
            let (keyPackage, pubkey_package) = frost::keys::dkg::part3(
                &round2_secret_package.clone(),
                &self.round1_group_packages,
                &self.round2_group_packages,
            )?;
            self.public_key_package = Some(pubkey_package);
            self.key_package = Some(keyPackage);
            Ok(())
        } else {
            Err(DKGError::MissingRound2SecretPackage)
        }
    }

    /** Signing Utils * */
    /// Created round 1 nonce commitments
    /// Returns the nonce commitment
    /// Or Err(_) if key package is not set
    pub fn create_round1_nonces(
        &mut self,
    ) -> Result<frost::round1::SigningCommitments, SigningError> {
        // TODO calling this should abort the current signing process
        if let Some(key_package) = &self.key_package {
            let mut rng = thread_rng();
            let nonces = frost::round1::commit(key_package.signing_share(), &mut rng);
            self.signer_nonces = Some(nonces.0);
            // caller does not need the nonce points
            // better not to return them
            return Ok(nonces.1)
        } else {
            Err(SigningError::MissingKeyPackage)
        }
    }

    /// Creates a round 2 signing share
    /// signs and provides partial signature
    /// Or Err(_) if key package is not set
    /// Or Err(_) if signer nonces are not set which should be set in `create_round1_nonces`
    pub fn create_round2_signing_share(
        &self,
        signing_package: &SigningPackage,
    ) -> Result<frost::round2::SignatureShare, SigningError> {
        if let Some(key_package) = &self.key_package {
            if let Some(signer_nonces) = &self.signer_nonces {
                let mut rng = thread_rng();
                let signature_share =
                    frost::round2::sign(signing_package, &signer_nonces, key_package)?;
                // TODO save signature
                Ok(signature_share)
            } else {
                Err(SigningError::MissingSignerNonces)
            }
        } else {
            Err(SigningError::MissingKeyPackage)
        }
    }

    /* Cordinator utilities */
    /// for Recieving round 1 commitments from signers
    pub fn add_new_nonce_commitment(
        &mut self,
        signing_commitment: frost::round1::SigningCommitments,
        peer_identifier: frost::Identifier,
    ) -> Result<(), CordinatorError> {
        if self.signing_commitmentments.len() == self.max_signers as usize {
            return Err(CordinatorError::ExceedingMaxNonceCommitments);
        }

        if self.signing_commitmentments.contains_key(&peer_identifier) {
            return Err(CordinatorError::DuplicateSigningCommitment);
        }

        self.signing_commitmentments.insert(peer_identifier, signing_commitment);
        Ok(())
    }

    /// For recieving round 2 signature shares from signers
    pub fn add_new_signature_share(
        &mut self,
        signature_share: frost::round2::SignatureShare,
        peer_identifier: frost::Identifier,
    ) -> Result<(), CordinatorError> {
        if self.signature_shares.len() == self.max_signers as usize {
            return Err(CordinatorError::ExceedingMaxNonceCommitments);
        }

        if self.signature_shares.contains_key(&peer_identifier) {
            return Err(CordinatorError::DuplicateSignatureShare);
        }

        self.signature_shares.insert(peer_identifier, signature_share);
        Ok(())
    }

    /// Creates a signing package given a message (bitcoin transaction)
    pub fn create_signing_package(&self, message: &[u8]) -> Result<SigningPackage, SigningError> {
        if let Some(key_package) = &self.key_package {
            let signing_package =
                frost::SigningPackage::new(self.signing_commitmentments.clone(), message);
            Ok(signing_package)
        } else {
            Err(SigningError::MissingKeyPackage)
        }
    }

    /// Aggregates signing shares
    /// returns Secp256k1 signature or
    /// Err(_) if pubkey package is not set
    pub fn aggregate_signing_shares(
        &self,
        signing_package: &SigningPackage,
    ) -> Result<frost::Signature, SigningError> {
        if let Some(pubkey_package) = &self.public_key_package {
            let agg = frost::aggregate(signing_package, &self.signature_shares, &pubkey_package)?;
            Ok(agg)
        } else {
            return Err(SigningError::MissingPublicKeyPackage)
        }
    }
}

mod test {
    use super::*;

    fn dkg(
        dkg1: &mut FrostState,
        dkg2: &mut FrostState,
        id1: frost::Identifier,
        id2: frost::Identifier,
    ) {
        let min_signer = 2;
        let max_signer = 2;
        dkg1.generate_personal_round1_package().expect("generate round 1");
        dkg2.generate_personal_round1_package().expect("generate round 1");

        // Send round1 packages
        let round1_package1 = dkg1.personal_round_1.clone().expect("round 1 package");
        let round1_package2 = dkg2.personal_round_1.clone().expect("round 1 package");

        dkg1.set_round1_group_package(id2.clone(), round1_package2.clone());
        dkg2.set_round1_group_package(id1.clone(), round1_package1.clone());

        assert_eq!(dkg1.round1_group_packages.len(), 1);
        assert_eq!(dkg2.round1_group_packages.len(), 1);

        // generate round 2 package and share with peers
        let round2_packages1 = dkg1.generate_personal_round2_package().expect("generate round 2");
        let round2_packages2 = dkg2.generate_personal_round2_package().expect("generate round 2");

        dkg1.add_participant_round2(
            id2.clone(),
            round2_packages2.get(&id1).clone().expect("round 2").clone(),
        );
        dkg2.add_participant_round2(
            id1.clone(),
            round2_packages1.get(&id2).clone().expect("round 2").clone(),
        );

        assert_eq!(dkg1.round2_group_packages.len(), 1);
        assert_eq!(dkg2.round2_group_packages.len(), 1);

        // create public key package
        dkg1.create_pubkey_package().expect("create public key package");
        dkg2.create_pubkey_package().expect("create public key package");
    }

    #[test]
    fn dkg_flow() {
        let min_signer = 2;
        let max_signer = 2;

        let id1 = frost::Identifier::try_from(1u16).expect("identifier");
        let id2 = frost::Identifier::try_from(2u16).expect("identifier");
        assert_ne!(id1, id2);

        let mut dkg1 = FrostState::new(min_signer, max_signer, id1);
        let mut dkg2 = FrostState::new(min_signer, max_signer, id2);
        dkg(&mut dkg1, &mut dkg2, id1, id2);

        assert_eq!(dkg1.public_key_package, dkg2.public_key_package);
    }

    #[test]
    fn signing_flow() {
        let min_signer = 2;
        let max_signer = 2;

        let id1 = frost::Identifier::try_from(1u16).expect("identifier");
        let id2 = frost::Identifier::try_from(2u16).expect("identifier");
        assert_ne!(id1, id2);

        let mut dkg1 = FrostState::new(min_signer, max_signer, id1);
        let mut dkg2 = FrostState::new(min_signer, max_signer, id2);
        dkg(&mut dkg1, &mut dkg2, id1, id2);
        // Create cordinator

        let cord_id = frost::Identifier::try_from(3u16).expect("identifier");
        let mut cord = FrostState::new(min_signer, max_signer, cord_id);

        //////////////////
        // Round 1
        //////////////////

        // Create round 1 nonces
        let signing_commit1 = dkg1.create_round1_nonces().expect("create round 1 nonces");
        let signing_commit2 = dkg2.create_round1_nonces().expect("create round 1 nonces");

        // share with cord
        cord.add_new_nonce_commitment(signing_commit1, id1).expect("add new nonce commitment");
        cord.add_new_nonce_commitment(signing_commit2, id2).expect("add new nonce commitment");

        //////////////////
        // Round 2
        //////////////////
        cord.key_package = dkg1.key_package.clone();
        cord.public_key_package = dkg1.public_key_package.clone();
        let message = [1u8, 2u8, 3u8];
        let signing_package =
            cord.create_signing_package(&message).expect("create signing package");

        // share message with signers
        let signature_share1 = dkg1
            .create_round2_signing_share(&signing_package)
            .expect("create round 2 signing share");
        let signature_share2 = dkg2
            .create_round2_signing_share(&signing_package)
            .expect("create round 2 signing share");

        // share with cord
        cord.add_new_signature_share(signature_share1, id1).expect("add new signature share");
        cord.add_new_signature_share(signature_share2, id2).expect("add new signature share");

        // Aggregate signatures
        let signature =
            cord.aggregate_signing_shares(&signing_package).expect("aggregate signatures");

        // Verify message
        cord.public_key_package
            .unwrap()
            .verifying_key()
            .verify(&message, &signature)
            .expect("verify signature");
    }
}
