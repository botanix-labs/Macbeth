use std::{collections::BTreeMap, io, path::Path};

use bitcoin::{OutPoint, TxOut};
use ciborium;
use frost_secp256k1_tr as frost;
use rand::thread_rng;
use serde::{Deserialize, Serialize};
use sled;
use thiserror::Error;

use crate::util::OutPointExt;

/// sled tree id for the utxos tree.
const TREE_UTXOS: &[u8; 5] = b"utxos";
const TREE_KEYS: &[u8; 4] = b"keys";

/// Datastructure for storing key information relevant to a particular multiset
/// Specifically we need to keep track of the following:
/// key_package
/// public_key_package
/// our personal identifier
/// round1 packages (if DKG is occuring)
/// round2 packages (if DKG is occuring)
/// Any secret packages (either personal or group) should be calculated on the fly
/// and not stored in the database
#[derive(Debug, Serialize, Deserialize)]
pub struct Keys {
    pub min_signers: u16,
    pub max_signers: u16,
    personal_identifier: frost::Identifier,
    personal_round_1: Option<frost::keys::dkg::round1::Package>,
    #[serde(skip)]
    personal_secret_package: Option<frost::keys::dkg::round1::SecretPackage>,
    round1_group_packages: BTreeMap<frost::Identifier, frost::keys::dkg::round1::Package>,
    round2_group_packages: BTreeMap<frost::Identifier, frost::keys::dkg::round2::Package>,
    #[serde(skip)]
    round2_secret_package: Option<frost::keys::dkg::round2::SecretPackage>,
    key_package: Option<frost::keys::KeyPackage>,
    public_key_package: Option<frost::keys::PublicKeyPackage>,
}

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
    _db: sled::Db,

    /// A tree of UTXOs.
    ///
    /// Indexed by serialized outpoint.
    utxos: sled::Tree,

    /// A tree of key information
    keys: sled::Tree,
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

impl Keys {
    pub fn new(min_signers: u16, max_signers: u16, personal_identifier: frost::Identifier) -> Keys {
        Keys {
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
        }
    }

    pub fn set_personal_round_1(&mut self, round1: frost::keys::dkg::round1::Package) {
        self.personal_round_1 = Some(round1);
    }

    pub fn set_round1_group_package(
        &mut self,
        identifier: frost::Identifier,
        round1: frost::keys::dkg::round1::Package,
    ) {
        self.round1_group_packages.insert(identifier, round1);
    }

    pub fn set_round2_group_package(
        &mut self,
        identifier: frost::Identifier,
        round2: frost::keys::dkg::round2::Package,
    ) {
        self.round2_group_packages.insert(identifier, round2);
    }

    /** Round 1 utils * */
    pub fn generate_personal_round1_package(&mut self) -> Result<(), frost::Error> {
        let mut rng = thread_rng();
        let (secret_package, round1_personal_package) = frost::keys::dkg::part1(
            self.personal_identifier,
            self.max_signers,
            self.min_signers,
            rng,
        )?;

        self.personal_round_1 = Some(round1_personal_package);
        self.personal_secret_package = Some(secret_package);

        Ok(())
    }

    pub fn add_participant_round1(
        &mut self,
        peer_identifier: frost::Identifier,
        peer_round1_package: frost::keys::dkg::round1::Package,
    ) {
        self.round1_group_packages.insert(peer_identifier, peer_round1_package);
    }

    /** Round 2 utils */
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

    pub fn add_participant_round2(
        &mut self,
        peer_identifier: frost::Identifier,
        peer_round2_package: frost::keys::dkg::round2::Package,
    ) {
        self.round2_group_packages.insert(peer_identifier, peer_round2_package);
    }

    /** Round 3 Utils * */
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
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Db, sled::Error> {
        let db = sled::open(path)?;
        Ok(Db { keys: db.open_tree(&TREE_KEYS)?, utxos: db.open_tree(&TREE_UTXOS)?, _db: db })
    }

    pub fn flush(&self) -> Result<(), Error> {
        self.utxos.flush()?;
        self.keys.flush()?;
        Ok(())
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
}

impl From<sled::transaction::TransactionError<sled::Error>> for Error {
    fn from(e: sled::transaction::TransactionError<sled::Error>) -> Error {
        match e {
            sled::transaction::TransactionError::Abort(e) => Error::Db(e),
            sled::transaction::TransactionError::Storage(e) => Error::Db(e),
        }
    }
}
