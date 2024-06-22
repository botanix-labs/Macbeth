use std::{array::TryFromSliceError, collections::BTreeMap, io, path::Path};

use bitcoin::{
    consensus::encode::Encodable,
    hashes::{sha256, Hash},
    psbt::{self, Psbt},
    BlockHash, OutPoint, TxOut,
};
use ciborium;
use client::SigningStatus;
use frost_secp256k1_tr as frost;
use miniscript::psbt::PsbtExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{txindex, util::OutPointExt};

/// sled tree id for the utxos tree.
const TREE_UTXOS: &[u8; 5] = b"utxos";
const TREE_ROUND1_DKG_PERSONAL_PACKAGE: &[u8; 5] = b"r1dkg";
const TREE_ROUND2_DKG_PERSONAL_PACKAGE: &[u8; 5] = b"r2dkg";
const TREE_PUBKEY_PACKAGE: &[u8; 5] = b"pubpk";
const TREE_KEY_PACKAGE: &[u8; 5] = b"keypk";
const TREE_PSBT: &[u8; 4] = b"psbt";
/// sled tree id for the pending txs
const TREE_PENDING_TXS: &[u8; 10] = b"pendingtxs";

/// sled key for the UTXO merkle tree root
const KEY_UTXO_MERKLE_ROOT: &[u8; 4] = b"root";

/// sled key for storing the latest finalized block of the txindex.
const KEY_TXINDEX_TIP: &[u8; 10] = b"txindextip";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Utxo {
    #[serde(skip)]
    pub outpoint: OutPoint,
    pub output: TxOut,
    /// If this is a pegin UTXO, the user's pegin address.
    pub eth_address: Option<[u8; 20]>,
}

impl Utxo {
    pub fn new(outpoint: OutPoint, output: TxOut, eth_address: Option<[u8; 20]>) -> Self {
        Utxo { outpoint, output, eth_address }
    }
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

    /// A tree of PSBTs
    ///
    /// Indexed by signing_session_id
    /// round 1 signing commitments and round 2 partial signatures are commited inside the psbt
    /// Only relevant for the coordinator
    psbt: sled::Tree,

    /// A tree of pending txs, serialized as the [txindex::Tx] format.
    ///
    /// Indexed by txid.
    pending_txs: sled::Tree,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Db, sled::Error> {
        let db = sled::open(path)?;
        Ok(Db {
            utxos: db.open_tree(TREE_UTXOS)?,
            round1_dkg_packages: db.open_tree(TREE_ROUND1_DKG_PERSONAL_PACKAGE)?,
            round2_dkg_packages: db.open_tree(TREE_ROUND2_DKG_PERSONAL_PACKAGE)?,
            psbt: db.open_tree(TREE_PSBT)?,
            pending_txs: db.open_tree(TREE_PENDING_TXS)?,
            db,
        })
    }

    pub fn flush(&self) -> Result<(), Error> {
        self.db.flush()?;
        self.utxos.flush()?;
        self.round1_dkg_packages.flush()?;
        self.round2_dkg_packages.flush()?;
        self.psbt.flush()?;
        self.pending_txs.flush()?;
        Ok(())
    }

    /// Adds a PSBT to the database.
    pub fn update_psbt(&self, signing_session_id: &[u8; 32], psbt: &Psbt) -> Result<(), Error> {
        let mut bytes = Vec::new();
        if let Some(b) = self.psbt.get(&signing_session_id[..])? {
            // if there is an existing psbt then we merge the new psbt with the existing one
            let mut existing_psbt = ciborium::from_reader::<Psbt, _>(b.as_ref())?;
            existing_psbt.combine(psbt.clone())?;
            ciborium::into_writer(&existing_psbt, &mut bytes).expect("writing to buffer");
        } else {
            ciborium::into_writer(psbt, &mut bytes).expect("writing to buffer");
        }
        self.psbt.insert(&signing_session_id[..], &bytes[..])?;
        Ok(())
    }

    /// Get PSBT from the database.
    /// Returns None if the PSBT is not found.
    /// Rertieves psbt using signing_session_id
    pub fn get_psbt(&self, signing_session_id: &[u8; 32]) -> Result<Option<Psbt>, Error> {
        if let Some(b) = self.psbt.get(&signing_session_id[..])? {
            let ret = ciborium::from_reader::<Psbt, _>(b.as_ref())?;
            Ok(Some(ret))
        } else {
            Ok(None)
        }
    }

    /// Get signing session ids from db
    pub fn get_session_ids(&self, max_results: u32) -> Result<Vec<[u8; 32]>, Error> {
        let mut ret = Vec::new();
        let mut results = 0;
        for res in self.psbt.iter() {
            let (k, _) = res?;
            let signing_session_id: [u8; 32] =
                k.to_vec().as_slice().try_into().map_err(Error::Serialization)?;
            results += 1;
            if max_results == results {
                break;
            }
            ret.push(signing_session_id);
        }
        Ok(ret)
    }

    pub fn get_signing_status(
        &self,
        signing_session_id: &[u8; 32],
    ) -> Result<SigningStatus, Error> {
        match self.get_psbt(signing_session_id)? {
            Some(psbt) => {
                let secp = bitcoin::secp256k1::Secp256k1::new();
                match psbt.finalize(&secp) {
                    Ok(_) => Ok(SigningStatus::Finalized),
                    Err(_) => Ok(SigningStatus::Running),
                }
            }
            None => Ok(SigningStatus::Failed), // session id deleted/expired
        }
    }

    /// Retrieves the public key package stored in the database, if available.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some(public_key_package))` if the public key package is found in the database.
    /// Returns `Ok(None)` if the public key package is not found.
    /// Returns `Err` in case of deserialization or other errors.
    pub fn get_public_key_package(&self) -> Result<Option<frost::keys::PublicKeyPackage>, Error> {
        if let Some(b) = self.db.get(TREE_PUBKEY_PACKAGE)? {
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
        if let Some(b) = self.db.get(TREE_KEY_PACKAGE)? {
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

        self.db.insert(TREE_KEY_PACKAGE, &bytes[..])?;
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

        self.db.insert(TREE_PUBKEY_PACKAGE, &bytes[..])?;
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
                k.to_vec().as_slice().try_into().map_err(Error::Serialization)?;

            let peer_id = frost::Identifier::deserialize(&peer_id_bytes)
                .map_err(Error::FrostSerialization)?;

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
                k.to_vec().as_slice().try_into().map_err(Error::Serialization)?;

            let peer_id = frost::Identifier::deserialize(&peer_id_bytes)
                .map_err(Error::FrostSerialization)?;

            let dkg_round1 =
                ciborium::from_reader::<frost::keys::dkg::round1::Package, _>(v.as_ref())?;
            ret.insert(peer_id, dkg_round1);
        }
        Ok(ret)
    }

    /* UTXO specific DB functions */
    pub fn get_utxo(&self, op: OutPoint) -> Result<Option<Utxo>, Error> {
        if let Some(b) = self.utxos.get(op.to_bytes())? {
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
        if !self.utxos.contains_key(op.to_bytes())? {
            let mut bytes = Vec::new();
            ciborium::into_writer(&utxo, &mut bytes).expect("writing to buffer");
            self.utxos.insert(op.to_bytes(), &bytes[..])?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Retrieves all utxos from the database.
    pub fn get_all_utxos(&self) -> Result<Vec<Utxo>, Error> {
        let mut utxos = vec![];
        for res in self.utxos.iter() {
            let (_k, v) = res?;
            let utxo: Utxo = ciborium::de::from_reader(v.as_ref()).expect("corrupt db: utxo");
            utxos.push(utxo);
        }
        Ok(utxos)
    }

    pub fn store_pending_tx(&self, tx: &txindex::Tx) -> Result<(), Error> {
        let mut bytes = Vec::new();
        ciborium::into_writer(tx, &mut bytes).expect("writing to buffer");
        self.pending_txs.insert(tx.txid, &bytes[..])?;
        Ok(())
    }

    pub fn get_pending_txs(&self) -> Result<Vec<txindex::Tx>, Error> {
        let mut ret = Vec::new();
        for res in self.pending_txs.iter() {
            let (_k, v) = res?;
            let tx = ciborium::de::from_reader(v.as_ref()).expect("corrupt db: pending tx");
            ret.push(tx);
        }
        Ok(ret)
    }

    pub fn store_txindex_finalized_block(&self, block_hash: BlockHash) -> Result<(), Error> {
        self.db.insert(KEY_TXINDEX_TIP, &block_hash.to_byte_array())?;
        Ok(())
    }

    pub fn get_txindex_finalized_block(&self) -> Result<Option<BlockHash>, Error> {
        Ok(self
            .db
            .get(KEY_TXINDEX_TIP)?
            .map(|t| BlockHash::from_slice(&t).expect("corrupt db: txindex block hash")))
    }

    /// Stores the consensus Merkle root of all spendable UTXOs.
    pub fn update_utxo_merkle_root(&self) -> Result<(), Error> {
        let mut utxos = self
            .iter_utxos()
            .map(|u| {
                let mut engine = sha256::Hash::engine();
                u?.outpoint.consensus_encode(&mut engine).expect("engine don't error");
                Ok(sha256::Hash::from_engine(engine))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        utxos.sort();
        if utxos.is_empty() {
            return Ok(());
        }

        let root = bitcoin::merkle_tree::calculate_root(utxos.into_iter()).expect("not empty");
        self.db.insert(KEY_UTXO_MERKLE_ROOT, root.to_byte_array().to_vec())?;
        Ok(())
    }

    /// Retrieves the consensus Merkle root of all spendable UTXOs.
    pub fn get_utxo_merkle_root(&self) -> Result<Option<sha256::Hash>, Error> {
        Ok(self.db.get(KEY_UTXO_MERKLE_ROOT)?.map(|b| {
            sha256::Hash::from_slice(&b).expect("corrupt db: Merkle root should be 32 bytes")
        }))
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
    #[error("PSBT error: {0}")]
    Psbt(#[from] psbt::Error),
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

#[cfg(test)]
mod tests {
    use crate::test::create_tx;

    use super::*;
    use tempfile::TempDir;

    fn setup_db() -> (Db, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db = Db::open(temp_dir.path()).unwrap();
        (db, temp_dir)
    }

    #[test]
    fn test_reading_session_ids() {
        let (db, _temp_dir) = setup_db();

        let tx = create_tx(2);
        let psbt = Psbt::from_unsigned_tx(tx).unwrap();
        let signing_session_id: [u8; 32] = [0; 32];
        db.update_psbt(&signing_session_id, &psbt).unwrap();
        db.flush().unwrap();

        let signing_session_ids = db.get_session_ids(10).unwrap();
        assert!(signing_session_ids.len() == 1);
    }

    #[test]
    fn test_getting_session_id_status() {
        let (db, _temp_dir) = setup_db();

        let tx = create_tx(2);
        let psbt = Psbt::from_unsigned_tx(tx).unwrap();
        let signing_session_id: [u8; 32] = [0; 32];
        db.update_psbt(&signing_session_id, &psbt).unwrap();
        db.flush().unwrap();

        let signing_session_id = db.get_session_ids(10).unwrap().first().cloned().unwrap();
        let signing_status = db.get_signing_status(&signing_session_id).unwrap();
        assert!(signing_status == SigningStatus::Running);
    }

    #[test]
    fn test_get_utxo_merkle_root_not_found() {
        let (db, _temp_dir) = setup_db();

        // Do not store anything and directly attempt to retrieve
        let retrieved_merkle_root = db.get_utxo_merkle_root().unwrap();
        assert!(
            retrieved_merkle_root.is_none(),
            "Should not retrieve a Merkle root when none has been stored."
        );
    }
}
