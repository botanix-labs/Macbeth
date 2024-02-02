use std::{array::TryFromSliceError, collections::BTreeMap, io, path::Path};

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

#[derive(Debug, Serialize, Deserialize)]
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
    /// Indexed by peer id
    round1_signing_packages: sled::Tree,

    /// A tree of round 2 partial signatures
    ///
    /// Indexed by peer id
    round2_signing_packages: sled::Tree,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Db, sled::Error> {
        let db = sled::open(path)?;
        Ok(Db {
            utxos: db.open_tree(&TREE_UTXOS)?,
            round1_dkg_packages: db.open_tree(ROUND1_DKG_PERSONAL_PACKAGE)?,
            round2_dkg_packages: db.open_tree(ROUND2_DKG_PERSONAL_PACKAGE)?,
            round1_signing_packages: db.open_tree(ROUND1_SIGNING_PACKAGES)?,
            round2_signing_packages: db.open_tree(ROUND1_SIGNING_PACKAGES)?,
            db,
        })
    }

    pub fn flush(&self) -> Result<(), Error> {
        self.utxos.flush()?;
        self.db.flush()?;
        self.round1_dkg_packages.flush()?;
        self.round2_dkg_packages.flush()?;
        self.round1_signing_packages.flush()?;
        self.round2_signing_packages.flush()?;
        Ok(())
    }

    pub fn add_round2_signing(
        &self,
        peer_id: frost::Identifier,
        signing_round2: frost::round2::SignatureShare,
    ) -> Result<bool, Error> {
        let peer_id_bytes = peer_id.serialize();

        if self.round2_signing_packages.contains_key(&peer_id_bytes[..])? {
            return Ok(false);
        }
        let mut bytes = Vec::new();
        ciborium::into_writer(&signing_round2, &mut bytes).expect("writing to buffer");
        self.round2_signing_packages.insert(&peer_id_bytes[..], &bytes[..])?;
        Ok(true)
    }

    pub fn add_round1_signing(
        &self,
        peer_id: frost::Identifier,
        signing_round1: frost::round1::SigningCommitments,
    ) -> Result<bool, Error> {
        let peer_id_bytes = peer_id.serialize();

        if self.round1_signing_packages.contains_key(&peer_id_bytes[..])? {
            return Ok(false);
        }
        let mut bytes = Vec::new();
        ciborium::into_writer(&signing_round1, &mut bytes).expect("writing to buffer");
        self.round1_signing_packages.insert(&peer_id_bytes[..], &bytes[..])?;
        Ok(true)
    }

    pub fn get_round1_signing_packages(
        &self,
    ) -> Result<BTreeMap<frost::Identifier, frost::round1::SigningCommitments>, Error> {
        let mut ret = BTreeMap::new();
        for res in self.round1_signing_packages.iter() {
            let (k, v) = res?;
            let peer_id_bytes: [u8; 32] =
                k.to_vec().as_slice().try_into().map_err(|e| Error::Serialization(e))?;

            let peer_id = frost::Identifier::deserialize(&peer_id_bytes)
                .map_err(|e| Error::FrostSerialization(e))?;
            let signing_round1 =
                ciborium::from_reader::<frost::round1::SigningCommitments, _>(v.as_ref())?;
            ret.insert(peer_id, signing_round1);
        }
        Ok(ret)
    }

    pub fn get_round2_signing_packages(
        &self,
    ) -> Result<BTreeMap<frost::Identifier, frost::round2::SignatureShare>, Error> {
        let mut ret = BTreeMap::new();
        for res in self.round2_signing_packages.iter() {
            let (k, v) = res?;
            let peer_id_bytes: [u8; 32] =
                k.to_vec().as_slice().try_into().map_err(|e| Error::Serialization(e))?;

            let peer_id = frost::Identifier::deserialize(&peer_id_bytes)
                .map_err(|e| Error::FrostSerialization(e))?;
            let signing_round2 =
                ciborium::from_reader::<frost::round2::SignatureShare, _>(v.as_ref())?;
            ret.insert(peer_id, signing_round2);
        }
        Ok(ret)
    }

    pub fn get_public_key_package(&self) -> Result<Option<frost::keys::PublicKeyPackage>, Error> {
        if let Some(b) = self.db.get(PUBKEY_PACKAGE)? {
            let ret = ciborium::from_reader::<frost::keys::PublicKeyPackage, _>(b.as_ref())?;
            Ok(Some(ret))
        } else {
            Ok(None)
        }
    }

    pub fn get_key_package(&self) -> Result<Option<frost::keys::KeyPackage>, Error> {
        if let Some(b) = self.db.get(KEY_PACKAGE)? {
            let ret = ciborium::from_reader::<frost::keys::KeyPackage, _>(b.as_ref())?;
            Ok(Some(ret))
        } else {
            Ok(None)
        }
    }

    pub fn set_key_package(&self, key_package: frost::keys::KeyPackage) -> Result<(), Error> {
        let mut bytes = Vec::new();
        ciborium::into_writer(&key_package, &mut bytes).expect("writing to buffer");

        self.db.insert(KEY_PACKAGE, &bytes[..])?;
        Ok(())
    }

    pub fn set_pubkey_package(
        &self,
        pk_package: frost::keys::PublicKeyPackage,
    ) -> Result<(), Error> {
        let mut bytes = Vec::new();
        ciborium::into_writer(&pk_package, &mut bytes).expect("writing to buffer");

        self.db.insert(PUBKEY_PACKAGE, &bytes[..])?;
        Ok(())
    }

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
        new: impl Iterator<Item = &'a Utxo> + Clone,
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
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("internal BD error")]
    Db(#[from] sled::Error),
    #[error("data corruption error")]
    DataCorruption(#[from] ciborium::de::Error<io::Error>),
    #[error("Frost serialization error {0}")]
    FrostSerialization(#[from] frost::Error),
    #[error("Serialization error {0}")]
    Serialization(#[from] TryFromSliceError),
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
