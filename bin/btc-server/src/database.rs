use std::{
    array::TryFromSliceError,
    collections::BTreeMap,
    io::{self},
    path::Path,
};

use crate::util::OutPointExt;
use bitcoin::{OutPoint, TxOut};
use ciborium;
use frost_secp256k1_tr as frost;

use serde::{Deserialize, Serialize};
use sled;
use thiserror::Error;

/// sled tree id for the utxos tree.
const TREE_UTXOS: &[u8; 5] = b"utxos";
const ROUND1_DKG_PERSONAL_PACKAGE: &[u8; 5] = b"r1dkg";
const ROUND2_DKG_PERSONAL_PACKAGE: &[u8; 5] = b"r2dkg";
const PUBKEY_PACKAGE: &[u8; 5] = b"pubpk";
const KEY_PACKAGE: &[u8; 5] = b"keypk";
const ROUND1_SIGNING_PACKAGES: &[u8; 5] = b"r1sig";
const ROUND2_SIGNING_PACKAGES: &[u8; 5] = b"r2sig";
const SIGNING_PACKAGES: &[u8; 5] = b"signp";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Utxo {
    #[serde(skip)]
    pub outpoint: OutPoint,
    pub output: TxOut,
    /// If this is a pegin UTXO, the user's pegin address.
    pub eth_address: Option<[u8; 20]>,
}

pub struct Db {
    /// NB a db is also a "default tree" so maybe here we could store some
    /// metadata if we wanted to. But I think it makes sense to have a different
    /// tree for the UTXOs.
    db: sled::Db,

    /// A tree of UTXOs.
    ///
    /// Indexed by serialized outpoint.
    utxos: sled::Tree,

    /// A tree of round 1 dkg commitments
    ///
    /// Indexed by peer id
    round1_dkg_packages: sled::Tree,

    /// A tree of round 1 dkg commitments
    ///
    /// Indexed by peer id
    round2_dkg_packages: sled::Tree,

    /// A tree of round 1 signing commitments
    ///
    /// Indexed by signing_session_id
    /// Values are a map of peer_id -> Vec<round1::SigningCommitments>
    /// Where each Vec is a list of commitments for each input of the transaction
    /// Only relevant for the coordinator
    round1_signing_packages: sled::Tree,

    /// A tree of round 2 partial signatures
    ///
    /// Indexed by signing_session_id
    /// Values are a map of peer_id -> Vec<round2::SignatureShare>
    /// Where each Vec is a list of partial signatures for each input of the transaction
    /// Only relevant for the coordinator
    round2_signing_packages: sled::Tree,

    // A tree of signing packages
    // Indexed by signing_session_id
    // Only relevant for the coordinator
    signing_packages: sled::Tree,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Db, sled::Error> {
        let db = sled::open(path)?;
        Ok(Db {
            utxos: db.open_tree(&TREE_UTXOS)?,
            round1_dkg_packages: db.open_tree(ROUND1_DKG_PERSONAL_PACKAGE)?,
            round2_dkg_packages: db.open_tree(ROUND2_DKG_PERSONAL_PACKAGE)?,
            round1_signing_packages: db.open_tree(ROUND1_SIGNING_PACKAGES)?,
            round2_signing_packages: db.open_tree(ROUND2_SIGNING_PACKAGES)?,
            signing_packages: db.open_tree(SIGNING_PACKAGES)?,
            db,
        })
    }

    // Temporary function to clear the db
    pub fn _clear(&self) -> Result<(), Error> {
        self.round1_signing_packages.clear()?;
        self.round2_signing_packages.clear()?;
        self.signing_packages.clear()?;
        Ok(())
    }

    pub fn flush(&self) -> Result<(), Error> {
        self.utxos.flush()?;
        self.db.flush()?;
        self.round1_dkg_packages.flush()?;
        self.round2_dkg_packages.flush()?;
        self.round1_signing_packages.flush()?;
        self.round2_signing_packages.flush()?;
        self.signing_packages.flush()?;
        Ok(())
    }

    /// Adds a vec of signing package to the collection for a given signing session.
    /// Each signing package is associated with a specific input of the final transaction.
    ///
    /// # Arguments
    ///
    /// * `signing_session_id` - A 32-byte array representing the unique identifier of the signing
    ///   session.
    /// * `signing_packages` - A vector of `frost::SigningPackage` to be added to the signing
    ///   session.
    ///
    /// # Returns
    ///
    /// Returns `Ok(true)` if the signing package was added successfully, `Ok(false)` if the signing
    /// session already contains a signing package with the given identifier. Returns `Err` in
    /// case of other errors.
    pub fn add_signing_package(
        &self,
        signing_session_id: &[u8; 32],
        signing_packages: Vec<frost::SigningPackage>,
    ) -> Result<bool, Error> {
        if self.signing_packages.contains_key(&signing_session_id[..])? {
            return Ok(false);
        }

        let mut bytes = Vec::new();
        ciborium::into_writer(&signing_packages, &mut bytes).expect("writing to buffer");
        self.signing_packages.insert(&signing_session_id[..], &bytes[..])?;
        Ok(true)
    }

    /// Gets the signing package associated with the given signing session identifier.
    ///
    /// # Arguments
    ///
    /// * `signing_session_id` - A 32-byte array representing the unique identifier of the signing
    ///   session.
    ///
    /// # Returns
    ///
    /// Returns a vector of `frost::SigningPackage` for the given signing session identifier.
    /// If the signing session does not exist, an empty vector is returned.
    ///
    /// # Errors
    ///
    /// Returns an `Err` if there is an issue deserializing the signing packages.
    pub fn get_signing_package(
        &self,
        signing_session_id: &[u8; 32],
    ) -> Result<Vec<frost::SigningPackage>, Error> {
        if let Some(b) = self.signing_packages.get(&signing_session_id[..])? {
            let ret = ciborium::from_reader::<Vec<frost::SigningPackage>, _>(b.as_ref())?;
            Ok(ret)
        } else {
            Ok(vec![])
        }
    }

    // Adds round 2 signing information to the specified signing session.
    ///
    /// # Arguments
    ///
    /// * `signing_session_id` - A 32-byte array representing the unique identifier of the signing
    ///   session.
    /// * `peer_id` - The frost identifier of the peer contributing to the signing session.
    /// * `signing_round2` - A vector of `frost::round2::SignatureShare` containing the round 2
    ///   signatures.
    ///
    /// # Returns
    ///
    /// Returns `Ok(true)` if the round 2 signing information was added successfully.
    /// Returns `Err` in case of other errors.
    pub fn add_round2_signing(
        &self,
        signing_session_id: &[u8; 32],
        peer_id: &frost::Identifier,
        signing_round2: &Vec<frost::round2::SignatureShare>,
    ) -> Result<bool, Error> {
        // for each input, we have a map of peer_id -> partial sig
        // loop throw each map (repersenting a partial sigs for input) and add this peer's signature
        let mut existing_partial_sigs = self.get_round2_signing_packages(signing_session_id)?;
        // If there are no existing partial signatures, initialize the vector
        if existing_partial_sigs.is_empty() {
            existing_partial_sigs.extend(
                signing_round2
                    .iter()
                    .map(|partial_sigs| BTreeMap::from_iter(vec![(*peer_id, *partial_sigs)])),
            );
        } else {
            // Update existing partial signatures
            for (sigs, round2_partial_sig) in
                existing_partial_sigs.iter_mut().zip(signing_round2.iter())
            {
                // Skip if the peer_id already has a signature
                if !sigs.contains_key(peer_id) {
                    sigs.insert(*peer_id, *round2_partial_sig);
                }
            }
        }
        let mut bytes = Vec::new();
        ciborium::into_writer(&existing_partial_sigs, &mut bytes).expect("writing to buffer");
        self.round2_signing_packages.insert(&signing_session_id[..], &bytes[..])?;

        Ok(true)
    }

    /// Adds round 1 signing data for a specific signing session
    ///
    /// # Arguments
    ///
    /// * `signing_session_id` - A fixed-size array of 32 bytes representing the unique identifier
    ///   of the signing session.
    /// * `peer_id` - An identifier representing the peer associated with the signing data.
    /// * `signing_commitments` - A vector containing round 1 signing commitments for the specified
    ///   session. Each commitment is associated with a specific input of the final transaction.
    ///
    /// # Returns
    ///
    /// Returns a `Result` indicating success (`Ok(true)`) if the round 1 signing data is
    /// successfully added. Returns `Ok(false)` if the signing session ID already exists in
    /// storage. Returns an `Err` variant if there are errors in the process.
    pub fn add_round1_signing(
        &self,
        signing_session_id: &[u8; 32],
        peer_id: frost::Identifier,
        signing_commitments: Vec<frost::round1::SigningCommitments>,
    ) -> Result<bool, Error> {
        let mut round1_commitments = self.get_round1_signing_packages(signing_session_id)?;
        // check if this frost id already has a commitment
        if round1_commitments.contains_key(&peer_id) {
            return Ok(false);
        }

        round1_commitments.insert(peer_id, signing_commitments);
        let mut bytes = Vec::new();
        ciborium::into_writer(&round1_commitments, &mut bytes).expect("writing to buffer");

        self.round1_signing_packages.insert(&signing_session_id[..], &bytes[..])?;
        Ok(true)
    }

    /// Retrieves round 1 signing packages associated with a specific signing session from storage.
    ///
    /// # Arguments
    ///
    /// * `signing_session_id` - A fixed-size array of 32 bytes representing the unique identifier
    ///   of the signing session.
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing a `BTreeMap` where the keys are peer identifiers and the
    /// values are vectors of round 1 signing commitments associated with the provided signing
    /// session ID. Returns `Ok(BTreeMap::new())` if no data is found for the specified signing
    /// session ID. Returns an `Err` variant if there are errors in the process.
    pub fn get_round1_signing_packages(
        &self,
        signing_session_id: &[u8; 32],
    ) -> Result<BTreeMap<frost::Identifier, Vec<frost::round1::SigningCommitments>>, Error> {
        // let mut ret = BTreeMap::new();
        for res in self.round1_signing_packages.iter() {
            let (k, v) = res?;
            let signing_session_id_key: [u8; 32] =
                k.to_vec().as_slice().try_into().map_err(|e| Error::Serialization(e))?;
            if signing_session_id_key != *signing_session_id {
                continue;
            }
            let signing_commitments = ciborium::from_reader::<
                BTreeMap<frost::Identifier, Vec<frost::round1::SigningCommitments>>,
                _,
            >(&mut v.as_ref())?;

            return Ok(signing_commitments);
        }
        Ok(BTreeMap::new())
    }

    /// Retrieves the round 2 signing packages associated with the specified signing session.
    ///
    /// # Arguments
    ///
    /// * `signing_session_id` - A 32-byte array representing the unique identifier of the signing
    ///   session.
    ///
    /// # Returns
    ///
    /// Returns a vector of `BTreeMap` where each map represents the partial signatures for a peer
    /// in the specified signing session. If no matching signing session is found, an empty vector
    /// is returned.
    pub fn get_round2_signing_packages(
        &self,
        signing_session_id: &[u8; 32],
    ) -> Result<Vec<BTreeMap<frost::Identifier, frost::round2::SignatureShare>>, Error> {
        for res in self.round2_signing_packages.iter() {
            let (k, v) = res?;

            let signing_session_id_key: [u8; 32] =
                k.to_vec().as_slice().try_into().map_err(|e| Error::Serialization(e))?;

            if signing_session_id_key != *signing_session_id {
                continue;
            }

            let partial_sig_set = ciborium::from_reader::<
                Vec<BTreeMap<frost::Identifier, frost::round2::SignatureShare>>,
                _,
            >(v.as_ref())?;
            return Ok(partial_sig_set);
        }
        // Could not find partial sigs for this signing session id
        // TODO Should we throw instead
        Ok(vec![])
    }

    /// Retrieves the public key package stored in the database, if available.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some(public_key_package))` if the public key package is found in the database.
    /// Returns `Ok(None)` if the public key package is not found.
    /// Returns `Err` in case of deserialization or other errors.
    pub fn get_public_key_package(&self) -> Result<Option<frost::keys::PublicKeyPackage>, Error> {
        if let Some(b) = self.db.get(PUBKEY_PACKAGE)? {
            let ret = ciborium::from_reader::<frost::keys::PublicKeyPackage, _>(b.as_ref())?;
            Ok(Some(ret))
        } else {
            Ok(None)
        }
    }

    /// Retrieves the key package stored in the database, if available.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some(key_package))` if the key package is found in the database.
    /// Returns `Ok(None)` if the key package is not found.
    /// Returns `Err` in case of deserialization or other errors.
    pub fn get_key_package(&self) -> Result<Option<frost::keys::KeyPackage>, Error> {
        if let Some(b) = self.db.get(KEY_PACKAGE)? {
            let ret = ciborium::from_reader::<frost::keys::KeyPackage, _>(b.as_ref())?;
            Ok(Some(ret))
        } else {
            Ok(None)
        }
    }

    /// Sets the key package in the database.
    ///
    /// # Arguments
    ///
    /// * `key_package` - The `frost::keys::KeyPackage` to be stored in the database.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the key package is successfully stored in the database.
    /// Returns `Err` in case of serialization or other errors.
    pub fn set_key_package(&self, key_package: frost::keys::KeyPackage) -> Result<(), Error> {
        let mut bytes = Vec::new();
        ciborium::into_writer(&key_package, &mut bytes).expect("writing to buffer");

        self.db.insert(KEY_PACKAGE, &bytes[..])?;
        Ok(())
    }

    /// Sets the public key package in the database.
    ///
    /// # Arguments
    ///
    /// * `pk_package` - The `frost::keys::PublicKeyPackage` to be stored in the database.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the public key package is successfully stored in the database.
    /// Returns `Err` in case of serialization or other errors.
    pub fn set_pubkey_package(
        &self,
        pk_package: frost::keys::PublicKeyPackage,
    ) -> Result<(), Error> {
        let mut bytes = Vec::new();
        ciborium::into_writer(&pk_package, &mut bytes).expect("writing to buffer");

        self.db.insert(PUBKEY_PACKAGE, &bytes[..])?;
        Ok(())
    }

    /// Adds a round 2 DKG package for a specific peer.
    ///
    /// # Arguments
    ///
    /// * `peer_id` - The `frost::Identifier` of the peer for whom the round 2 DKG package is being
    ///   added.
    /// * `dkg_round2_package` - The `frost::keys::dkg::round2::Package` representing the round 2
    ///   DKG package.
    ///
    /// # Returns
    ///
    /// Returns `Ok(true)` if the round 2 DKG package is successfully added for the peer.
    /// Returns `Ok(false)` if a round 2 DKG package for the specified peer already exists.
    /// Returns `Err` in case of serialization or other errors.
    pub fn add_round2_dkg(
        &self,
        peer_id: frost::Identifier,
        dkg_round2_package: frost::keys::dkg::round2::Package,
    ) -> Result<bool, Error> {
        let peer_id_bytes = peer_id.serialize();

        if self.round2_dkg_packages.contains_key(&peer_id_bytes[..])? {
            return Ok(false);
        }
        let mut bytes = Vec::new();

        ciborium::into_writer(&dkg_round2_package, &mut bytes).expect("writing to buffer");
        self.round2_dkg_packages.insert(&peer_id_bytes[..], &bytes[..])?;
        Ok(true)
    }

    /// Adds a round 1 DKG package for a specific peer.
    ///
    /// # Arguments
    ///
    /// * `peer_id` - The `frost::Identifier` of the peer for whom the round 1 DKG package is being
    ///   added.
    /// * `dkg_round1` - The `frost::keys::dkg::round1::Package` representing the round 1 DKG
    ///   package.
    ///
    /// # Returns
    ///
    /// Returns `Ok(true)` if the round 1 DKG package is successfully added for the peer.
    /// Returns `Ok(false)` if a round 1 DKG package for the specified peer already exists.
    /// Returns `Err` in case of serialization or other errors.
    pub fn add_round1_dkg(
        &self,
        peer_id: frost::Identifier,
        dkg_round1: frost::keys::dkg::round1::Package,
    ) -> Result<bool, Error> {
        let peer_id_bytes = peer_id.serialize();

        if self.round1_dkg_packages.contains_key(&peer_id_bytes[..])? {
            return Ok(false);
        }
        let mut bytes = Vec::new();
        ciborium::into_writer(&dkg_round1, &mut bytes).expect("writing to buffer");
        self.round1_dkg_packages.insert(&peer_id_bytes[..], &bytes[..])?;
        Ok(true)
    }

    /// Retrieves the round 2 DKG (Distributed Key Generation) packages stored in the database.
    ///
    /// # Returns
    ///
    /// Returns a `BTreeMap` where keys are `frost::Identifier` representing peers and values are
    /// `frost::keys::dkg::round2::Package` representing the associated round 2 DKG packages.
    /// If no round 2 DKG packages are found, an empty `BTreeMap` is returned.
    ///
    /// # Errors
    ///
    /// Returns an `Err` if there is an issue deserializing the DKG packages or handling
    /// serialization errors.
    pub fn get_round2_dkg_packages(
        &self,
    ) -> Result<BTreeMap<frost::Identifier, frost::keys::dkg::round2::Package>, Error> {
        let mut ret = BTreeMap::new();
        for res in self.round2_dkg_packages.iter() {
            let (k, v) = res?;
            let peer_id_bytes: [u8; 32] =
                k.to_vec().as_slice().try_into().map_err(|e| Error::Serialization(e))?;

            let peer_id = frost::Identifier::deserialize(&peer_id_bytes)
                .map_err(|e| Error::FrostSerialization(e))?;

            let dkg_round2 =
                ciborium::from_reader::<frost::keys::dkg::round2::Package, _>(v.as_ref())?;
            ret.insert(peer_id, dkg_round2);
        }
        Ok(ret)
    }

    /// Retrieves the round 1 DKG (Distributed Key Generation) packages stored in the database.
    ///
    /// # Returns
    ///
    /// Returns a `BTreeMap` where keys are `frost::Identifier` representing peers and values are
    /// `frost::keys::dkg::round1::Package` representing the associated round 1 DKG packages.
    /// If no round 1 DKG packages are found, an empty `BTreeMap` is returned.
    ///
    /// # Errors
    ///
    /// Returns an `Err` if there is an issue deserializing the DKG packages or handling
    /// serialization errors.
    pub fn get_round1_dkg_packages(
        &self,
    ) -> Result<BTreeMap<frost::Identifier, frost::keys::dkg::round1::Package>, Error> {
        let mut ret = BTreeMap::new();
        for res in self.round1_dkg_packages.iter() {
            let (k, v) = res?;
            let peer_id_bytes: [u8; 32] =
                k.to_vec().as_slice().try_into().map_err(|e| Error::Serialization(e))?;

            let peer_id = frost::Identifier::deserialize(&peer_id_bytes)
                .map_err(|e| Error::FrostSerialization(e))?;

            let dkg_round1 =
                ciborium::from_reader::<frost::keys::dkg::round1::Package, _>(v.as_ref())?;
            ret.insert(peer_id, dkg_round1);
        }
        Ok(ret)
    }

    /* UTXO specific DB functions */
    pub fn get_utxo(&self, op: OutPoint) -> Result<Option<Utxo>, Error> {
        if let Some(b) = self.utxos.get(&op.to_bytes())? {
            let mut ret = ciborium::from_reader::<Utxo, _>(b.as_ref())?;
            ret.outpoint = op;
            Ok(Some(ret))
        } else {
            Ok(None)
        }
    }

    pub fn iter_utxos(&self) -> impl Iterator<Item = Result<Utxo, Error>> {
        self.utxos.iter().map(|res| {
            let (k, v) = res?;
            let mut ret = ciborium::from_reader::<Utxo, _>(v.as_ref())?;
            ret.outpoint = OutPoint::from_slice(&k).expect("db very broken");
            Ok(ret)
        })
    }

    pub fn store_utxo(&self, utxo: &Utxo) -> Result<bool, Error> {
        let op = utxo.outpoint;
        if !self.utxos.contains_key(&op.to_bytes())? {
            let mut bytes = Vec::new();
            ciborium::into_writer(&utxo, &mut bytes).expect("writing to buffer");
            self.utxos.insert(&op.to_bytes(), &bytes[..])?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Add new utxos and remove some utxos in one atomic transaction.
    pub fn add_remove_utxos<'a>(
        &self,
        remove: impl Iterator<Item = OutPoint> + Clone,
        new: impl Iterator<Item = Utxo> + Clone,
    ) -> Result<(), Error> {
        // NB the clones on the args is because the closure in the
        // transaction can be called multiple times in the case where
        // the transaction is aborted because of a conflict.
        // But since it's outpoints (small) and references (very small),
        // the clone operation is really cheap.

        self.utxos.transaction(move |utxos| {
            for r in remove.clone() {
                utxos.remove(&r.to_bytes()[..])?;
            }
            for n in new.clone() {
                let mut bytes = Vec::new();
                ciborium::into_writer(&n, &mut bytes).expect("writing to buffer");
                utxos.insert(&n.outpoint.to_bytes()[..], &bytes[..])?;
            }
            Ok(())
        })?;
        Ok(())
    }

    /// Retrieves all utxos from the database.
    pub async fn get_all_utxos(&self) -> Result<Vec<Utxo>, Error> {
        let mut utxos = vec![];
        for res in self.utxos.iter() {
            let (k, v) = res?; // Handle the Result here
            let utxo: Utxo = ciborium::de::from_reader(v.as_ref()).expect("decoding");
            utxos.push(utxo);
        }
        Ok(utxos)
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("internal DB error")]
    Db(#[from] sled::Error),
    #[error("data corruption error")]
    DataCorruption(#[from] ciborium::de::Error<io::Error>),
    #[error("Frost serialization error {0}")]
    FrostSerialization(#[from] frost::Error),
    #[error("Serialization error {0}")]
    Serialization(#[from] TryFromSliceError),
    #[error("bitcoin serialization error {0}")]
    BitcoinSerialization(#[from] bitcoin::consensus::encode::Error),
}

impl From<sled::transaction::TransactionError<sled::Error>> for Error {
    fn from(e: sled::transaction::TransactionError<sled::Error>) -> Error {
        match e {
            sled::transaction::TransactionError::Abort(e) => Error::Db(e),
            sled::transaction::TransactionError::Storage(e) => Error::Db(e),
        }
    }
}

// To make it easier to return tonic status error from the callers
impl From<Error> for tonic::Status {
    fn from(e: Error) -> tonic::Status {
        tonic::Status::internal(e.to_string())
    }
}
