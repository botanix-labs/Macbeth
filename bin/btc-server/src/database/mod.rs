use std::{
    collections::BTreeMap,
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    pegout_id::PegoutId,
    pegout_scheduler::{self},
    rpc::{OutPoint as RpcOutPoint, ScriptBuf as RpcScriptBuf, TxOut as RpcTxOut, Utxo as RpcUtxo},
    util::{parse_eth_address, OutPointExt},
};
use bitcoin::{
    consensus::encode::Encodable,
    hashes::{sha256, Hash},
    psbt::Psbt,
    Amount, BlockHash, OutPoint, ScriptBuf, TxOut, Txid,
};
use btc_server_client::SigningStatus;
use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit, Nonce};
use frost_secp256k1_tr as frost;
use futures::Stream;
use log::{info, warn};
use miniscript::psbt::PsbtExt;
use serde::{Deserialize, Serialize};
use sled::transaction::{ConflictableTransactionError, TransactionError};
pub mod error;
pub mod version;
pub use error::Error;
use version::UtxoVersion;
use zeroize::Zeroizing;

/// sled tree id for the utxos tree.
const TREE_UTXOS: &[u8; 5] = b"utxos";
const TREE_ROUND1_DKG_PERSONAL_PACKAGE: &[u8; 5] = b"r1dkg";
const TREE_ROUND2_DKG_PERSONAL_PACKAGE: &[u8; 5] = b"r2dkg";
const TREE_PUBKEY_PACKAGE: &[u8; 5] = b"pubpk";
const TREE_KEY_PACKAGE: &[u8; 5] = b"keypk";
const TREE_KEY_PACKAGES: &[u8; 6] = b"keypks";
const TREE_PUBKEY_PACKAGES: &[u8; 6] = b"pubpks";
const TREE_PSBT: &[u8; 4] = b"psbt";
const TREE_FINALIZED_PEGOUT_IDS: &[u8; 4] = b"pids";
/// sled tree id for the pending txs
const TREE_TRACKED_TXS: &[u8; 10] = b"trackedtxs";

/// sled key for the UTXO merkle tree root
const KEY_UTXO_MERKLE_ROOT: &[u8; 4] = b"root";

/// sled key for tracked Tx merkle root
const KEY_TRACKED_TX_MERKLE_ROOT: &[u8; 5] = b"troot";

/// sled key for pending pegouts merkle root
const KEY_PENDING_PEGOUTS_MERKLE_ROOT: &[u8; 5] = b"proot";

/// sled key for storing the latest finalized block of the txindex.
const KEY_PEGOUTMGR_TIP: &[u8; 12] = b"pegoutmgrtip";

/// sled key for finalized pegout ids
const KEY_FINALIZED_PEGOUT_IDS_MERKLE_ROOT: &[u8; 9] = b"pegoutids";

/// sled tree for pending pegout requests
const TREE_PENDING_PEGOUTS: &[u8; 7] = b"pegouts";

/// Sliding window duration in seconds (90 days)
const RETENTION_WINDOW_SECONDS: u64 = 90 * 24 * 60 * 60;

/// Multisig id reserved for the legacy (pre-dynafed) key package.
const LEGACY_MULTISIG_ID: u32 = 0;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Utxo {
    // This is skipped during serialization because the db key is the outpoint so its redundant.
    #[serde(skip)]
    pub outpoint: OutPoint,
    pub output: TxOut,
    /// If this is a pegin UTXO, the user's pegin address.
    pub eth_address: Option<[u8; 20]>,
    #[serde(default)]
    /// The version of the UTXO.
    pub version: u32,
}

impl Utxo {
    pub fn new(
        outpoint: OutPoint,
        output: TxOut,
        eth_address: Option<[u8; 20]>,
        version: Option<UtxoVersion>,
    ) -> Self {
        Utxo { outpoint, output, eth_address, version: version.unwrap_or(UtxoVersion::V1) as u32 }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct FinalizedPegout {
    /// The pegout id
    pub id: PegoutId,
    /// The Botanix block number.
    pub block_number: u64,
    /// The timestamp of when the pegout was stored in the db, if available.
    /// A pegout is stored when the pegout request is initiated on the L2.
    #[serde(default)]
    pub timestamp: Option<u64>,
}

impl FinalizedPegout {
    pub fn new(id: PegoutId, block_number: u64, timestamp: Option<u64>) -> Self {
        FinalizedPegout { id, block_number, timestamp }
    }
}

/// Current version for [`ExportedKeyPackage`] (future-reserved).
pub const EXPORTED_PACKAGE_VERSION: u16 = 0;

/// Encrypted key package export format for secure backup and transfer.
///
/// This structure contains both secret and public key packages encrypted with a
/// passphrase-derived key. A single random nonce is used for both encryption
/// operations, with two separately derived keys.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ExportedKeyPackage {
    /// Version indicator (future-reserved), currently it's
    /// [`EXPORTED_PACKAGE_VERSION`].
    pub version: u16,
    /// Random 96-bit nonce used for both encryption operations, in plaintext.
    pub iv: [u8; 12],
    /// Encrypted FROST secret key package. Contains the encrypted secret key
    /// material with authentication tag.
    pub enc_key_package: Vec<u8>,
    /// Encrypted FROST public key package. Contains the encrypted public key
    /// material with authentication tag.
    pub enc_pk_package: Vec<u8>,
}

#[derive(Clone)]
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
    /// round 1 signing commitments and round 2 partial signatures are committed inside the psbt
    /// Only relevant for the coordinator
    psbt: sled::Tree,

    /// A tree of tracked txs, serialized as the [pegoutScheduler::Tx] format.
    ///
    /// Indexed by txid.
    tracked_txs: sled::Tree,

    /// Finalized PegoutIds
    finalized_pegout_ids: sled::Tree,

    /// A tree of pending pegout requests, serialized as the [pegouts::PegoutRequest] format.
    ///
    /// Indexed by the [PegoutRequest::id] inspector.
    pending_pegouts: sled::Tree,

    /// A tree of FROST secret key packages (multi-key format).
    ///
    /// Indexed by multisig_id (u32).
    key_packages: sled::Tree,

    /// A tree of FROST public key packages (multi-key format).
    ///
    /// Indexed by multisig_id (u32).
    pubkey_packages: sled::Tree,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Db, sled::Error> {
        let db = sled::open(path)?;
        Ok(Db {
            utxos: db.open_tree(TREE_UTXOS)?,
            round1_dkg_packages: db.open_tree(TREE_ROUND1_DKG_PERSONAL_PACKAGE)?,
            round2_dkg_packages: db.open_tree(TREE_ROUND2_DKG_PERSONAL_PACKAGE)?,
            psbt: db.open_tree(TREE_PSBT)?,
            tracked_txs: db.open_tree(TREE_TRACKED_TXS)?,
            pending_pegouts: db.open_tree(TREE_PENDING_PEGOUTS)?,
            finalized_pegout_ids: db.open_tree(TREE_FINALIZED_PEGOUT_IDS)?,
            key_packages: db.open_tree(TREE_KEY_PACKAGES)?,
            pubkey_packages: db.open_tree(TREE_PUBKEY_PACKAGES)?,
            db,
        })
    }

    pub fn flush(&self) -> Result<(), Error> {
        self.db.flush()?;
        self.utxos.flush()?;
        self.round1_dkg_packages.flush()?;
        self.round2_dkg_packages.flush()?;
        self.psbt.flush()?;
        self.tracked_txs.flush()?;
        self.pending_pegouts.flush()?;
        self.finalized_pegout_ids.flush()?;
        self.key_packages.flush()?;
        self.pubkey_packages.flush()?;
        Ok(())
    }

    /// Adds a PSBT to the database.
    pub fn update_psbt(&self, signing_session_id: &[u8; 32], psbt: &Psbt) -> Result<usize, Error> {
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
        Ok(bytes.len())
    }

    /// Get PSBT from the database.
    /// Returns None if the PSBT is not found.
    /// Rertrieves psbt using signing_session_id
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

    /// Retrieves a key package by multisig_id from the multi-key storage.
    ///
    /// # Arguments
    ///
    /// * `multisig_id` - The u32 identifier for the multisig/federation.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some(key_package))` if the key package is found.
    /// Returns `Ok(None)` if the key package is not found.
    /// Returns `Err` in case of deserialization or other errors.
    pub fn get_key_package_by_id(
        &self,
        multisig_id: u32,
    ) -> Result<Option<frost::keys::KeyPackage>, Error> {
        let key = multisig_id.to_le_bytes();
        if let Some(b) = self.key_packages.get(&key)? {
            let ret = ciborium::from_reader::<frost::keys::KeyPackage, _>(b.as_ref())?;
            Ok(Some(ret))
        } else {
            Ok(None)
        }
    }

    /// Retrieves a public key package by multisig_id from the multi-key storage.
    ///
    /// # Arguments
    ///
    /// * `multisig_id` - The u32 identifier for the multisig/federation.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some(public_key_package))` if the public key package is found.
    /// Returns `Ok(None)` if the public key package is not found.
    /// Returns `Err` in case of deserialization or other errors.
    pub fn get_public_key_package_by_id(
        &self,
        multisig_id: u32,
    ) -> Result<Option<frost::keys::PublicKeyPackage>, Error> {
        let key = multisig_id.to_le_bytes();
        if let Some(b) = self.pubkey_packages.get(&key)? {
            let ret = ciborium::from_reader::<frost::keys::PublicKeyPackage, _>(b.as_ref())?;
            Ok(Some(ret))
        } else {
            Ok(None)
        }
    }

    /// Sets a key package by multisig_id in the multi-key storage.
    ///
    /// # Arguments
    ///
    /// * `multisig_id` - The u32 identifier for the multisig/federation.
    /// * `key_package` - The `frost::keys::KeyPackage` to be stored.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the key package is successfully stored.
    /// Returns `Err` in case of serialization or other errors.
    pub fn set_key_package_by_id(
        &self,
        multisig_id: u32,
        key_package: frost::keys::KeyPackage,
    ) -> Result<(), Error> {
        let key = multisig_id.to_le_bytes();
        let mut bytes = Vec::new();
        ciborium::into_writer(&key_package, &mut bytes).expect("writing to buffer");

        self.key_packages.insert(&key, &bytes[..])?;
        Ok(())
    }

    /// Sets a public key package by multisig_id in the multi-key storage.
    ///
    /// # Arguments
    ///
    /// * `multisig_id` - The u32 identifier for the multisig/federation.
    /// * `pk_package` - The `frost::keys::PublicKeyPackage` to be stored.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the public key package is successfully stored.
    /// Returns `Err` in case of serialization or other errors.
    pub fn set_pubkey_package_by_id(
        &self,
        multisig_id: u32,
        pk_package: frost::keys::PublicKeyPackage,
    ) -> Result<(), Error> {
        let key = multisig_id.to_le_bytes();
        let mut bytes = Vec::new();
        ciborium::into_writer(&pk_package, &mut bytes).expect("writing to buffer");

        self.pubkey_packages.insert(&key, &bytes[..])?;
        Ok(())
    }

    /// Lists all multisig_ids that have key packages stored.
    ///
    /// # Returns
    ///
    /// Returns a `Vec<u32>` containing all multisig_ids that have key packages.
    /// Returns an empty vector if no key packages are found.
    /// Returns `Err` in case of database errors.
    pub fn list_multisig_ids(&self) -> Result<Vec<u32>, Error> {
        let mut ids = Vec::new();
        for res in self.key_packages.iter().keys() {
            let key_bytes = res?;
            if key_bytes.len() == 4 {
                let id = u32::from_le_bytes(key_bytes.as_ref().try_into()?);
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// Migrates legacy single-key storage to new multi-key storage format.
    ///
    /// Checks if key packages exist in the old storage location and migrates them
    /// to the new multi-key storage trees with `multisig_id = 0`. Skips if already
    /// migrated or no legacy keys found.
    ///
    /// # Returns
    ///
    /// Returns `Ok(true)` if migration was performed.
    /// Returns `Ok(false)` if migration was skipped.
    /// Returns `Err` in case of database or serialization errors.
    pub fn migrate_legacy_key_package(&self) -> Result<bool, Error> {
        // Check if new format already has key at multisig_id = 0
        if self.get_key_package_by_id(LEGACY_MULTISIG_ID)?.is_some() {
            info!(
                "Key migration skipped: key already exists in new multi-key format (multisig_id={})",
                LEGACY_MULTISIG_ID
            );
            return Ok(false);
        }

        // Check if old format has keys using existing methods
        let legacy_key_package = self.get_key_package()?;
        let legacy_pubkey_package = self.get_public_key_package()?;

        if let (Some(key_package), Some(pk_package)) = (legacy_key_package, legacy_pubkey_package) {
            info!("Starting key package migration from legacy single-key to multi-key format...");

            // Store in new format with multisig_id = 0
            self.set_key_package_by_id(LEGACY_MULTISIG_ID, key_package)?;
            self.set_pubkey_package_by_id(LEGACY_MULTISIG_ID, pk_package)?;

            info!("Key package migration completed successfully: legacy key migrated to multisig_id={}", LEGACY_MULTISIG_ID);
            Ok(true)
        } else {
            warn!("Key migration skipped: legacy key package or public key package not found");
            Ok(false)
        }
    }

    /// Exports the stored key packages as an encrypted, portable format.
    ///
    /// Creates an [`ExportedKeyPackage`] containing both the secret and public
    /// key packages encrypted with a passphrase-derived key. The export can be
    /// safely stored or transmitted since all sensitive material is encrypted
    /// with ChaCha20-Poly1305.
    ///
    /// # Security Design
    /// - Uses a single random nonce but derives separate encryption keys to prevent catastrophic
    ///   nonce reuse
    /// - Provides authenticated encryption; tampering will cause import to fail
    /// - Passphrase is combined with random salt (nonce) for key derivation resistance
    ///
    /// # Arguments
    /// * `passphrase` - User-provided passphrase for encryption. Should be strong as it's the
    ///   primary protection for the exported keys.
    ///
    /// # Returns
    /// * `Ok(Some(export))` - Successfully created encrypted export
    /// * `Ok(None)` - No key packages available to export
    /// * `Err(_)` - Database error during key retrieval
    pub fn export_key_package(
        &self,
        passphrase: Zeroizing<String>,
    ) -> Result<Option<ExportedKeyPackage>, Error> {
        let Some(key_package) = self.db.get(TREE_KEY_PACKAGE)? else {
            return Ok(None);
        };

        let Some(pk_package) = self.db.get(TREE_PUBKEY_PACKAGE)? else {
            return Ok(None);
        };

        // Validate retrieved packages.
        #[cfg(debug_assertions)]
        {
            ciborium::from_reader::<frost::keys::KeyPackage, _>(key_package.as_ref())
                .expect("bad key package");

            ciborium::from_reader::<frost::keys::PublicKeyPackage, _>(pk_package.as_ref())
                .expect("bad public key package");
        }

        // IMPORTANT: We randomly generate a single nonce, which is included in
        // the export and saved alongside the encrypted data in plaintext. Since
        // we encrypt the secret and public key packages separately with the
        // same nonce, we deterministically derive separate encryption keys for
        // each package using the Merlin transcript. This prevents catastrophic
        // nonce reuse while keeping the export format simple (only one nonce to
        // store).
        let iv: [u8; 12] = rand::random();
        let nonce = Nonce::from_slice(&iv);

        let mut t = merlin::Transcript::new(b"Botanix_Macbeth_BtcServer_ExportedKeyPackage");
        t.append_message(b"salt", nonce);
        t.append_message(b"passphrase", passphrase.as_bytes());

        // Scope the `master` key so we don't accidentally reuse it between
        // separate encryption operations.

        // Encrypt secret key package with derived key.
        let enc_key_package: Vec<u8> = {
            let mut master = Zeroizing::new([0; 32]);

            // Derive NEW master key for the secret key package.
            t.challenge_bytes(b"secret_key_package", master.as_mut_slice());
            debug_assert_ne!(master.as_slice(), [0; 32]);

            ChaCha20Poly1305::new_from_slice(master.as_slice())
                .expect("master key must be 32-bytes")
                .encrypt(nonce, key_package.as_ref())
                .expect("output buffer must be valid")
        };

        // Encrypt public key package with derived key.
        let enc_pk_package: Vec<u8> = {
            let mut master = Zeroizing::new([0; 32]);

            // Derive NEW master key for the public key package.
            t.challenge_bytes(b"public_key_package", master.as_mut_slice());
            debug_assert_ne!(master.as_slice(), [0; 32]);

            ChaCha20Poly1305::new_from_slice(master.as_mut_slice())
                .expect("master key must be 32-bytes")
                .encrypt(nonce, pk_package.as_ref())
                .expect("output buffer must be valid")
        };

        let export = ExportedKeyPackage {
            version: EXPORTED_PACKAGE_VERSION,
            iv,
            enc_key_package,
            enc_pk_package,
        };

        Ok(Some(export))
    }

    /// Imports and validates an encrypted key package export.
    ///
    /// Decrypts an [`ExportedKeyPackage`] using the provided passphrase and
    /// stores the recovered key packages in the database. The decrypted data is
    /// validated by deserializing it into typed Rust structs, ensuring it's
    /// well-formed.
    ///
    /// # Security Validation
    /// - Authenticated decryption prevents accepting tampered data
    /// - Deserialization validates the decrypted data structure
    /// - Wrong passphrase will cause decryption to fail
    ///
    /// # Arguments
    /// * `passphrase` - The same passphrase used during export
    /// * `export` - The encrypted key package export to import
    ///
    /// # Returns
    /// * `Ok(())` - Successfully imported and stored key packages
    /// * `Err(_)` - Decryption failed (wrong passphrase), malformed data, or database error
    pub fn import_key_package(
        &self,
        passphrase: Zeroizing<String>,
        export: ExportedKeyPackage,
    ) -> Result<(), Error> {
        if export.version != EXPORTED_PACKAGE_VERSION {
            return Err(Error::BadExportedPackageFormatVersion);
        }

        // Retrieve the nonce directly from the exported package.
        let nonce = Nonce::from_slice(&export.iv);

        let mut t = merlin::Transcript::new(b"Botanix_Macbeth_BtcServer_ExportedKeyPackage");
        t.append_message(b"salt", nonce);
        t.append_message(b"passphrase", passphrase.as_bytes());

        let mut master = Zeroizing::new([0u8; 32]);

        // Convert decrypted bytes into typed Rust structs rather than storing
        // the raw bytes directly. This serves as validation/sanity-check that
        // ensures the decrypted data is actually valid and not garbage.

        let key_package: frost::keys::KeyPackage = {
            t.challenge_bytes(b"secret_key_package", master.as_mut_slice());

            let b = ChaCha20Poly1305::new_from_slice(master.as_slice())
                .expect("master key must be 32-bytes")
                .decrypt(nonce, export.enc_key_package.as_slice())
                .map_err(|_| Error::BadDecryptionPassphrase)?;

            ciborium::from_reader(b.as_slice())?
        };

        let pk_package: frost::keys::PublicKeyPackage = {
            t.challenge_bytes(b"public_key_package", master.as_mut_slice());

            let b = ChaCha20Poly1305::new_from_slice(master.as_slice())
                .expect("master key must be 32-bytes")
                .decrypt(nonce, export.enc_pk_package.as_slice())
                .map_err(|_| Error::BadDecryptionPassphrase)?;

            ciborium::from_reader(b.as_slice())?
        };

        self.set_key_package(key_package)?;
        self.set_pubkey_package(pk_package)?;

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
    /// Returns `Ok(val > 0)` if the round 2 DKG package is successfully added for the peer.
    /// Returns `Ok(0)` if a round 2 DKG package for the specified peer already exists.
    /// Returns `Err` in case of serialization or other errors.
    pub fn add_round2_dkg(
        &self,
        peer_id: frost::Identifier,
        dkg_round2_package: frost::keys::dkg::round2::Package,
    ) -> Result<usize, Error> {
        let peer_id_bytes = peer_id.serialize();

        if self.round2_dkg_packages.contains_key(&peer_id_bytes[..])? {
            return Ok(0);
        }
        let mut bytes = Vec::new();

        ciborium::into_writer(&dkg_round2_package, &mut bytes).map_err(Error::CiboriumWrite)?;
        self.round2_dkg_packages.insert(&peer_id_bytes[..], &bytes[..])?;
        Ok(bytes.len())
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
    /// Returns `Ok(val > 0)` if the round 1 DKG package is successfully added for the peer.
    /// Returns `Ok(0)` if a round 1 DKG package for the specified peer already exists.
    /// Returns `Err` in case of serialization or other errors.
    pub fn add_round1_dkg(
        &self,
        peer_id: frost::Identifier,
        dkg_round1: frost::keys::dkg::round1::Package,
    ) -> Result<usize, Error> {
        let peer_id_bytes = peer_id.serialize();

        if self.round1_dkg_packages.contains_key(&peer_id_bytes[..])? {
            return Ok(0);
        }
        let mut bytes = Vec::new();
        ciborium::into_writer(&dkg_round1, &mut bytes).map_err(Error::CiboriumWrite)?;
        self.round1_dkg_packages.insert(&peer_id_bytes[..], &bytes[..])?;
        Ok(bytes.len())
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

    /// Remove a UTXO from the database
    pub fn remove_utxo(&self, op: &OutPoint) -> Result<(), Error> {
        self.utxos.remove(op.to_bytes())?;
        Ok(())
    }

    pub fn iter_utxos(&self) -> impl Iterator<Item = Result<Utxo, Error>> {
        self.utxos.iter().map(|res| {
            let (k, v) = res?;
            let mut ret = ciborium::from_reader::<Utxo, _>(v.as_ref())?;
            ret.outpoint = OutPoint::from_slice(&k).expect("db very broken");
            Ok(ret)
        })
    }

    pub fn store_utxos(&self, utxos: &[&Utxo]) -> Result<bool, Error> {
        match utxos.len() {
            0 => Ok(false),
            1 => self.store_utxo(utxos.first().unwrap()),
            _ => self.store_utxos_atomically(utxos),
        }
    }

    fn store_utxo(&self, utxo: &Utxo) -> Result<bool, Error> {
        let op = utxo.outpoint;
        if !self.utxos.contains_key(op.to_bytes())? {
            let mut bytes = Vec::new();
            ciborium::into_writer(&utxo, &mut bytes).map_err(Error::CiboriumWrite)?;
            self.utxos.insert(op.to_bytes(), &bytes[..])?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn store_utxos_atomically(&self, utxos: &[&Utxo]) -> Result<bool, Error> {
        self.utxos
            .transaction(|database_tx| {
                for utxo in utxos.iter() {
                    let op = utxo.outpoint;
                    if database_tx.get(op.to_bytes())?.is_none() {
                        let mut bytes = Vec::new();
                        ciborium::into_writer(&utxo, &mut bytes)
                            .map_err(Error::CiboriumWrite)
                            .expect("Ciborium error");
                        database_tx.insert(op.to_bytes().to_vec(), &bytes[..])?;
                    }
                }
                Ok::<(), ConflictableTransactionError>(())
            })
            .map_err(|e: TransactionError<_>| Error::Transaction(e.to_string()))
            .map(|_| true)
    }

    /// Resetting all utxos, and re-adding the functions arguments back in
    pub fn reset_utxos(&self, utxos: &[&Utxo]) -> Result<(), Error> {
        self.clear_utxos()?;
        self.store_utxos(utxos)?;
        Ok(())
    }

    /// Clears all utxos from the database.
    pub fn clear_utxos(&self) -> Result<(), Error> {
        Ok(self.utxos.clear()?)
    }

    /// Retrieves all utxos from the database.
    pub fn get_all_utxos(&self) -> Result<Vec<Utxo>, Error> {
        let mut utxos = vec![];
        for res in self.utxos.iter() {
            let (k, v) = res?;
            let outpoint: OutPoint = OutPoint::from_slice(&k).expect("corrupt db: outpoint");
            let mut utxo: Utxo = ciborium::de::from_reader(v.as_ref()).expect("corrupt db: utxo");
            utxo.outpoint = outpoint;
            utxos.push(utxo);
        }
        Ok(utxos)
    }

    /// Store a list of txs that we are tracking for the pegout scheduler.
    pub fn store_tracked_txs(&self, txs: &[&pegout_scheduler::Tx]) -> Result<(), Error> {
        match txs.len() {
            0 => Ok(()),
            1 => self.store_tracked_tx(txs.first().expect("to have tx")),
            _ => self.store_tracked_txs_atomically(txs),
        }
    }

    /// Store a list of txs that we are tracking for the pegout scheduler atomically.
    pub fn store_tracked_txs_atomically(&self, txs: &[&pegout_scheduler::Tx]) -> Result<(), Error> {
        self.tracked_txs
            .transaction(|database_tx| {
                for tx in txs.iter() {
                    let txid = tx.txid;
                    if database_tx.get(txid)?.is_none() {
                        let mut bytes = Vec::new();
                        ciborium::into_writer(tx, &mut bytes)
                            .map_err(Error::CiboriumWrite)
                            .expect("Ciborium error");
                        database_tx.insert(txid.to_byte_array().to_vec(), &bytes[..])?;
                    }
                }
                Ok::<(), ConflictableTransactionError>(())
            })
            .map_err(|e: TransactionError<_>| Error::Transaction(e.to_string()))?;
        Ok(())
    }

    /// Store a tx that we are tracking for the pegout scheduler.
    pub fn store_tracked_tx(&self, tx: &pegout_scheduler::Tx) -> Result<(), Error> {
        let mut bytes = Vec::new();
        ciborium::into_writer(tx, &mut bytes).map_err(Error::CiboriumWrite)?;
        self.tracked_txs.insert(tx.txid, &bytes[..])?;
        self.update_tracked_tx_merkle_root()?;
        Ok(())
    }

    /// Get list of txs that we are tracking for the pegout scheduler.
    pub fn get_tracked_txs(&self) -> Result<Vec<pegout_scheduler::Tx>, Error> {
        let mut ret = Vec::new();
        for res in self.tracked_txs.iter() {
            let (_k, v) = res?;
            let tx = ciborium::de::from_reader(v.as_ref()).expect("corrupt db: pending tx");
            ret.push(tx);
        }
        Ok(ret)
    }

    /// Stores the consensus Merkle root of all spendable UTXOs.
    pub fn update_tracked_tx_merkle_root(&self) -> Result<(), Error> {
        let mut tracked_txs = self
            .get_tracked_txs()?
            .iter()
            .map(|tx| {
                let mut engine = sha256::Hash::engine();
                tx.txid.consensus_encode(&mut engine).expect("engine don't error");
                Ok(sha256::Hash::from_engine(engine))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        tracked_txs.sort();
        if tracked_txs.is_empty() {
            return Ok(());
        }

        let root = bitcoin::merkle_tree::calculate_root(tracked_txs.into_iter())
            .ok_or(Error::EmptyMerkleRoot)?;
        self.db.insert(KEY_TRACKED_TX_MERKLE_ROOT, root.to_byte_array().to_vec())?;
        Ok(())
    }

    pub fn get_tracked_tx_merkle_root(&self) -> Result<Option<sha256::Hash>, Error> {
        Ok(self.db.get(KEY_TRACKED_TX_MERKLE_ROOT)?.map(|b| {
            sha256::Hash::from_slice(&b).expect("corrupt db: Merkle root should be 32 bytes")
        }))
    }

    pub fn remove_tracked_tx(&self, txid: &Txid) -> Result<(), Error> {
        self.tracked_txs.remove(txid)?;
        Ok(())
    }

    pub fn store_pegout_mgr_finalized_block(&self, block_hash: BlockHash) -> Result<(), Error> {
        self.db.insert(KEY_PEGOUTMGR_TIP, &block_hash.to_byte_array())?;
        Ok(())
    }

    pub fn get_pegout_mgr_finalized_block(&self) -> Result<Option<BlockHash>, Error> {
        Ok(self
            .db
            .get(KEY_PEGOUTMGR_TIP)?
            .map(|t| BlockHash::from_slice(&t).expect("corrupt db: pegout mgr block hash")))
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

        let root = bitcoin::merkle_tree::calculate_root(utxos.into_iter())
            .ok_or(Error::EmptyMerkleRoot)?;
        self.db.insert(KEY_UTXO_MERKLE_ROOT, root.to_byte_array().to_vec())?;
        Ok(())
    }

    /// Retrieves the consensus Merkle root of all spendable UTXOs.
    pub fn get_utxo_merkle_root(&self) -> Result<Option<sha256::Hash>, Error> {
        Ok(self.db.get(KEY_UTXO_MERKLE_ROOT)?.map(|b| {
            sha256::Hash::from_slice(&b).expect("corrupt db: Merkle root should be 32 bytes")
        }))
    }

    /// Store a list of pending pegouts
    pub fn store_pending_pegouts(
        &self,
        pegout_requests: &[&pegout_scheduler::PegoutRequest],
    ) -> Result<(), Error> {
        match pegout_requests.len() {
            0 => Ok(()),
            1 => self.store_pending_pegout(pegout_requests.first().expect("to have tx")),
            _ => self.store_pending_pegouts_atomically(pegout_requests),
        }
    }

    /// Store a pending pegout
    pub fn store_pending_pegout(&self, req: &pegout_scheduler::PegoutRequest) -> Result<(), Error> {
        let mut bytes = Vec::new();
        ciborium::into_writer(&req, &mut bytes).map_err(Error::CiboriumWrite)?;
        self.pending_pegouts.insert(req.id.as_bytes(), &bytes[..])?;
        self.update_pending_pegouts_merkle_root()?;
        Ok(())
    }

    /// Store a list of pending pegouts atomically
    pub fn store_pending_pegouts_atomically(
        &self,
        pegout_requests: &[&pegout_scheduler::PegoutRequest],
    ) -> Result<(), Error> {
        self.pending_pegouts
            .transaction(|database_tx| {
                for req in pegout_requests.iter() {
                    if database_tx.get(req.id.as_bytes())?.is_none() {
                        let mut bytes = Vec::new();
                        ciborium::into_writer(req, &mut bytes)
                            .map_err(Error::CiboriumWrite)
                            .expect("Ciborium error");
                        database_tx.insert(req.id.as_bytes().to_vec(), &bytes[..])?;
                    }
                }
                Ok::<(), ConflictableTransactionError>(())
            })
            .map_err(|e: TransactionError<_>| Error::Transaction(e.to_string()))?;
        self.update_pending_pegouts_merkle_root()?;
        Ok(())
    }

    /// Stores the consensus Merkle root of all pending pegouts.
    pub fn update_pending_pegouts_merkle_root(&self) -> Result<(), Error> {
        let mut pending_pegouts = self
            .get_pending_pegouts()?
            .iter()
            .map(|req| {
                let mut engine = sha256::Hash::engine();
                let pegout_id = req.id.as_bytes();
                let _ = engine.write(&pegout_id).expect("to write pegout id");
                Ok(sha256::Hash::from_engine(engine))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        pending_pegouts.sort();
        if pending_pegouts.is_empty() {
            return Ok(());
        }

        let root = bitcoin::merkle_tree::calculate_root(pending_pegouts.into_iter())
            .ok_or(Error::EmptyMerkleRoot)?;
        self.db.insert(KEY_PENDING_PEGOUTS_MERKLE_ROOT, root.to_byte_array().to_vec())?;
        Ok(())
    }

    /// Get pending pegouts merkle root
    pub fn get_pending_pegouts_merkle_root(&self) -> Result<Option<sha256::Hash>, Error> {
        Ok(self.db.get(KEY_PENDING_PEGOUTS_MERKLE_ROOT)?.map(|b| {
            sha256::Hash::from_slice(&b).expect("corrupt db: Merkle root should be 32 bytes")
        }))
    }

    /// Get a pending pegout by id
    #[allow(dead_code)]
    pub fn get_pending_pegout(
        &self,
        id: &PegoutId,
    ) -> Result<Option<pegout_scheduler::PegoutRequest>, Error> {
        Ok(self
            .pending_pegouts
            .get(id.as_bytes())?
            .map(|b| ciborium::de::from_reader(b.as_ref()).expect("corrupt db: pending pegout")))
    }

    /// Get all pending pegouts
    pub fn get_pending_pegouts(&self) -> Result<Vec<pegout_scheduler::PegoutRequest>, Error> {
        let mut ret = Vec::new();
        for res in self.pending_pegouts.iter() {
            let (_k, v) = res?;
            let tx = ciborium::de::from_reader(v.as_ref()).expect("corrupt db: pending tx");
            ret.push(tx);
        }
        Ok(ret)
    }

    /// Returns up to `max` pending pegouts, sorted by age in ascending order.
    /// Respectively, the oldest pegouts come first.
    pub fn coord_pending_pegouts(
        &self,
        max: usize,
    ) -> Result<Vec<pegout_scheduler::PegoutRequest>, Error> {
        let mut pegouts = self.get_pending_pegouts()?;
        pegouts.sort_by(|a, b| a.botanix_height.cmp(&b.botanix_height));

        if pegouts.len() < max {
            return Ok(pegouts);
        }

        Ok(pegouts.into_iter().take(max).collect())
    }

    /// Removes pending pegouts from the database.
    pub fn remove_pending_pegout(&self, pegout_ids: &[PegoutId]) -> Result<(), Error> {
        for pegout_id in pegout_ids.iter() {
            self.pending_pegouts.remove(&pegout_id.as_bytes()[..])?;
        }
        Ok(())
    }

    /// Resets all pending pegouts, and re-adding the functions arguments back in
    pub fn reset_pending_pegouts(
        &self,
        pegout_requests: &[&pegout_scheduler::PegoutRequest],
    ) -> Result<(), Error> {
        self.clear_pending_pegouts()?;
        self.store_pending_pegouts(pegout_requests)?;
        Ok(())
    }

    /// Clears all pending pegouts from the database.
    pub fn clear_pending_pegouts(&self) -> Result<(), Error> {
        Ok(self.pending_pegouts.clear()?)
    }

    /// Get all finalized pegouts
    /// Returns a vector of pegout requests that have been finalized.
    pub fn get_finalized_pegout_ids(&self) -> Result<Vec<FinalizedPegout>, Error> {
        let mut ret = Vec::new();
        for res in self.finalized_pegout_ids.iter() {
            let (_k, v) = res?;
            let tx = ciborium::de::from_reader(v.as_ref()).expect("corrupt db: pending tx");
            ret.push(tx);
        }
        Ok(ret)
    }

    /// Count all finalized pegout ids
    /// Returns a count of pegout requests that have been finalized.
    pub fn peek_finalized_pegout_ids(&self) -> Result<usize, Error> {
        Ok(self.finalized_pegout_ids.iter().count())
    }

    /// Get all finalized pegout ids via a stream
    /// Returns a vector of pegout chunks that have been finalized.
    pub fn get_finalized_pegout_ids_stream(
        &self,
        chunk_size: usize,
    ) -> impl Stream<Item = Result<(Vec<FinalizedPegout>, u64, u64), Error>> + Send + '_ + Sync
    {
        async_stream::stream! {
            let total_count = match self.peek_finalized_pegout_ids() {
                Ok(count) => count,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            let num_chunks = total_count.div_ceil(chunk_size) as u64;
            let mut chunk_index: u64 = 0;

            // get all keys first (this is efficient in sled)
            let all_keys: Vec<_> = match self.finalized_pegout_ids.iter().keys().collect() {
                Ok(keys) => keys,
                Err(e) => {
                    yield Err(e.into());
                    return;
                }
            };

            // process keys in chunks
            for key_chunk in all_keys.chunks(chunk_size) {
                let mut items = Vec::with_capacity(chunk_size);

                for key in key_chunk {
                    if let Ok(Some(value)) = self.finalized_pegout_ids.get(key) {
                        match ciborium::de::from_reader(value.as_ref()) {
                            Ok(tx) => items.push(tx),
                            Err(e) => {
                                yield Err(Error::DataCorruption(e));
                                return;
                            }
                        }
                    }
                }

                if !items.is_empty() {
                    yield Ok((items, chunk_index, num_chunks));
                    chunk_index += 1;
                }
            }
        }
    }

    /// Removes finalized pegout ids from the database.
    pub fn remove_finalized_pegout_ids(
        &self,
        finalized_pegout_ids: &[FinalizedPegout],
    ) -> Result<(), Error> {
        for pegout_id in finalized_pegout_ids.iter() {
            self.finalized_pegout_ids.remove(&pegout_id.id.as_bytes()[..])?;
        }
        Ok(())
    }

    /// Clears all finalized pegout ids from the database.
    pub fn clear_finalized_pegout_ids(&self) -> Result<(), Error> {
        Ok(self.finalized_pegout_ids.clear()?)
    }

    /// Resets all finalized pegout txs, and re-adding the functions arguments back in
    pub fn reset_finalized_pegout_ids(
        &self,
        finalized_pegout_ids: &[&FinalizedPegout],
    ) -> Result<(), Error> {
        self.clear_finalized_pegout_ids()?;
        self.store_finalized_pegout_ids(finalized_pegout_ids)?;
        Ok(())
    }

    /// Store a list of finalized pegout ids
    pub fn store_finalized_pegout_ids(
        &self,
        finalized_pegout_ids: &[&FinalizedPegout],
    ) -> Result<(), Error> {
        match finalized_pegout_ids.len() {
            0 => Ok(()),
            1 => self.store_finalized_pegout_id(
                finalized_pegout_ids.first().expect("to have pegout id"),
            ),
            _ => self.store_finalized_pegout_ids_atomically(finalized_pegout_ids),
        }
    }

    fn store_finalized_pegout_id(&self, pegout_id: &FinalizedPegout) -> Result<(), Error> {
        let mut bytes = Vec::new();
        ciborium::into_writer(&pegout_id, &mut bytes).map_err(Error::CiboriumWrite)?;
        self.finalized_pegout_ids.insert(pegout_id.id.as_bytes(), &bytes[..])?;
        Ok(())
    }

    /// Store a list of finalized pegout ids atomically
    pub fn store_finalized_pegout_ids_atomically(
        &self,
        pegout_ids_requests: &[&FinalizedPegout],
    ) -> Result<(), Error> {
        self.finalized_pegout_ids
            .transaction(|database_tx| {
                for req in pegout_ids_requests.iter() {
                    if database_tx.get(req.id.as_bytes())?.is_none() {
                        let mut bytes = Vec::new();
                        ciborium::into_writer(req, &mut bytes)
                            .map_err(Error::CiboriumWrite)
                            .expect("Ciborium error");
                        database_tx.insert(req.id.as_bytes().to_vec(), &bytes[..])?;
                    }
                }
                Ok::<(), ConflictableTransactionError>(())
            })
            .map_err(|e: TransactionError<_>| Error::Transaction(e.to_string()))?;
        Ok(())
    }

    /// Prunes the tree of finalized pegout ids that are older than a specified pruning window.
    ///
    /// Returns `Ok(())` if the pruning was successful.
    /// Returns an error if there was an issue with the database transaction or deserialization.
    pub fn prune_finalized_pegout_ids(&self) -> Result<(), Error> {
        // Calculate the timestamp for the pruning cutoff
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(Error::DurationSinceEpoch)?
            .as_secs();

        let cutoff_timestamp = now.saturating_sub(RETENTION_WINDOW_SECONDS);

        // We can't iterate through the finalized_pegout_ids tree inside the database transaction,
        // so we get them first then iterate through them inside the transaction.
        let finalized_pegouts = self.get_finalized_pegout_ids()?;

        self.finalized_pegout_ids
            .transaction(|database_tx| {
                for pegout in finalized_pegouts.iter() {
                    match pegout.timestamp {
                        Some(timestamp) => {
                            // Check if the entry is older than the cutoff
                            if timestamp < cutoff_timestamp {
                                info!("Pruning finalized pegout id: {:?}", pegout.id);
                                database_tx.remove(&pegout.id.as_bytes()[..])?;
                            }
                        }
                        None => {
                            // Clone and update the pegout with current timestamp
                            let mut pegout_with_timestamp = pegout.clone();
                            pegout_with_timestamp.timestamp = Some(now);

                            // Serialize and insert back into the database
                            let mut bytes = Vec::new();
                            ciborium::into_writer(&pegout_with_timestamp, &mut bytes).map_err(
                                |e| ConflictableTransactionError::Abort(Error::CiboriumWrite(e)),
                            )?;

                            info!("Updating finalized pegout id with timestamp: {:?}", pegout.id);
                            database_tx.insert(pegout.id.as_bytes().to_vec(), &bytes[..])?;
                            // No need to check if it should be pruned - it was just updated with a
                            // timestamp
                        }
                    }
                }

                Ok::<(), ConflictableTransactionError<Error>>(())
            })
            .map_err(|e: TransactionError<Error>| Error::Transaction(e.to_string()))?;

        Ok(())
    }

    pub fn iter_finalized_pegout_ids(
        &self,
    ) -> impl Iterator<Item = Result<FinalizedPegout, Error>> {
        self.finalized_pegout_ids.iter().map(|res| {
            let (_, v) = res?;
            let ret = ciborium::from_reader::<FinalizedPegout, _>(v.as_ref())?;
            Ok(ret)
        })
    }

    /// Stores the consensus Merkle root of all finalized pegout ids.
    pub fn update_finalized_pegout_ids_merkle_root(&self) -> Result<(), Error> {
        let mut finalized_pegout_ids = self
            .iter_finalized_pegout_ids()
            .map(|pegout_id| {
                let mut engine = sha256::Hash::engine();
                let pegout_id = pegout_id?;
                pegout_id.id.idx.consensus_encode(&mut engine).expect("engine don't error");
                pegout_id.id.txid.consensus_encode(&mut engine).expect("engine don't error");
                pegout_id.block_number.consensus_encode(&mut engine).expect("engine don't error");
                Ok(sha256::Hash::from_engine(engine))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        finalized_pegout_ids.sort();
        if finalized_pegout_ids.is_empty() {
            return Ok(());
        }

        let root = bitcoin::merkle_tree::calculate_root(finalized_pegout_ids.into_iter())
            .ok_or(Error::EmptyMerkleRoot)?;
        self.db.insert(KEY_FINALIZED_PEGOUT_IDS_MERKLE_ROOT, root.to_byte_array().to_vec())?;
        Ok(())
    }

    /// Retrieves the consensus Merkle root of all finalized pegout ids.
    pub fn get_finalized_pegout_ids_merkle_root(&self) -> Result<Option<sha256::Hash>, Error> {
        Ok(self.db.get(KEY_FINALIZED_PEGOUT_IDS_MERKLE_ROOT)?.map(|b| {
            sha256::Hash::from_slice(&b).expect("corrupt db: Merkle root should be 32 bytes")
        }))
    }

    /// Resets all tracked txs, and re-adding the functions arguments back in
    pub fn reset_tracked_txs(&self, tracked_txs: &[&pegout_scheduler::Tx]) -> Result<(), Error> {
        self.clear_tracked_txs()?;
        self.store_tracked_txs(tracked_txs)?;
        Ok(())
    }

    /// Clears all tracked txs from the database.
    pub fn clear_tracked_txs(&self) -> Result<(), Error> {
        Ok(self.tracked_txs.clear()?)
    }
}

fn parse_p2tr_script_with_fallback(script_bytes: &[u8]) -> ScriptBuf {
    // Parse as raw script bytes (expected format)
    let script_from_raw = ScriptBuf::from_bytes(script_bytes.to_vec());
    if script_from_raw.is_p2tr() {
        return script_from_raw;
    }

    // Fallback: Handle legacy consensus-encoded format (with length prefix)
    // Provides backwards compatibility in case other node services are still
    // using the old encoding format (see https://github.com/botanix-labs/Macbeth/pull/949)
    if let Ok(script_from_consensus) = bitcoin::consensus::deserialize::<ScriptBuf>(script_bytes) {
        warn!("Received RpcUtxo with legacy consensus-encoded script format");
        if script_from_consensus.is_p2tr() {
            return script_from_consensus;
        }
    }

    // If neither format produces a valid P2TR script, return the raw script
    // as it may be a non-p2tr script
    warn!("Received RpcUtxo with non-p2tr script format");
    script_from_raw
}

impl TryFrom<RpcUtxo> for Utxo {
    type Error = Error;

    fn try_from(value: RpcUtxo) -> Result<Self, Self::Error> {
        // outpoint
        let outpoint =
            value.outpoint.ok_or_else(|| Error::RpcToDbMap("Outpoint is None".to_string()))?;
        let txid = bitcoin::consensus::deserialize::<Txid>(&outpoint.txid)
            .map_err(|_| Error::RpcToDbMap("Unparsable Txid".to_string()))?;
        let vout = outpoint.vout;

        // txout
        let tx_out = value.output.ok_or_else(|| Error::RpcToDbMap("TxOut is None".to_string()))?;
        let tx_out_val = Amount::from_sat(tx_out.value);
        let script_pubkey = tx_out
            .script_pubkey
            .ok_or_else(|| Error::RpcToDbMap("Script Pub Key is None".to_string()))?;
        let script = parse_p2tr_script_with_fallback(&script_pubkey.script);

        // create the utxo
        Ok(Utxo::new(
            OutPoint::new(txid, vout),
            TxOut { value: tx_out_val, script_pubkey: script },
            if value.eth_address.is_empty() {
                None
            } else {
                Some(
                    parse_eth_address(value.eth_address).map_err(|_| {
                        Error::RpcToDbMap("Unparsable Ethereum Address".to_string())
                    })?,
                )
            },
            Some(UtxoVersion::V1),
        ))
    }
}

impl TryFrom<Utxo> for RpcUtxo {
    type Error = Error;

    fn try_from(item: Utxo) -> Result<Self, Self::Error> {
        // Use script bytes directly without additional consensus encoding
        // The script is already in the correct format from the database
        let script_pk = item.output.script_pubkey.to_bytes();

        Ok(RpcUtxo {
            outpoint: Some(RpcOutPoint {
                txid: AsRef::<[u8]>::as_ref(&item.outpoint.txid).to_vec(),
                vout: item.outpoint.vout,
            }),
            output: Some(RpcTxOut {
                value: item.output.value.to_sat(),
                script_pubkey: Some(RpcScriptBuf { script: script_pk }),
            }),
            eth_address: item.eth_address.map_or(String::new(), hex::encode),
        })
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use rand::{thread_rng, Rng};
    use tokio::pin;

    use crate::{
        pegout_scheduler::{PegoutRequest, Tx},
        test_utils::{
            create_random_pegout_id, create_tx, random_p2wpkh_scriptpubkey, setup_db,
            trusted_dealer_setup,
        },
    };
    use std::{collections::HashSet, time::SystemTime};

    use super::*;
    use crate::pegout_id::PegoutId;

    // Original structure (simulating old version)
    #[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
    struct OldFinalizedPegout {
        pub id: PegoutId,
        pub block_number: u64,
    }

    #[test]
    fn can_save_and_read_pegout_reqs() {
        let (db, _temp_dir) = setup_db();

        let pegout_id = PegoutId::new([0; 32], 0);
        let req = pegout_scheduler::PegoutRequest {
            id: pegout_id,
            spk: random_p2wpkh_scriptpubkey(),
            value: Amount::from_sat(1000),
            botanix_height: 1,
            timestamp: None,
        };
        db.store_pending_pegout(&req).unwrap();
        let pegouts = db.get_pending_pegouts().unwrap();
        assert_eq!(pegouts.len(), 1);
        let pegout_req = pegouts.get(0).unwrap();
        assert_eq!(pegout_req.id, req.id);
        assert_eq!(pegout_req.spk, req.spk);
        assert_eq!(pegout_req.value, req.value);
        assert_eq!(pegout_req.botanix_height, req.botanix_height);

        // Can retrieve by id
        let pegout_req = db.get_pending_pegout(&pegout_id).unwrap().unwrap();
        assert_eq!(pegout_req.id, req.id);
        assert_eq!(pegout_req.spk, req.spk);
        assert_eq!(pegout_req.value, req.value);
        assert_eq!(pegout_req.botanix_height, req.botanix_height);
    }

    #[test]
    fn can_coordinate_pending_pegouts_based_on_height() {
        let (db, _temp_dir) = setup_db();

        // Create pegouts with the appropriate height in reverse order;
        // pegout with id 0 is the newest, while Id 9 is the oldest.
        //
        // NOTE: We specifically reverse the order of botanix heights so we can
        // verify that the returned pegouts are sorted accordingly based on
        // height, not Id.
        for i in 0..10 {
            let pegout_id = PegoutId::new([i as u8; 32], 0);
            let req = pegout_scheduler::PegoutRequest {
                id: pegout_id,
                spk: random_p2wpkh_scriptpubkey(),
                value: Amount::from_sat(100_000),
                botanix_height: 50 - i,
                timestamp: None,
            };
            db.store_pending_pegout(&req).unwrap();
        }

        // Coordinate zero pegouts.
        let pegouts = db.coord_pending_pegouts(0).unwrap();
        assert!(pegouts.is_empty());

        // Coordinate 4 pegouts which are sorted from oldest to newest; pegout
        // with Id 9 comes first, followed by Id 8, etc.
        let pegouts = db.coord_pending_pegouts(4).unwrap();
        assert_eq!(pegouts.len(), 4);

        for i in 0..4 {
            let id = 9 - i;

            let pegout_id = PegoutId::new([id as u8; 32], 0);
            assert_eq!(pegouts[i].id, pegout_id);
            assert_eq!(pegouts[i].botanix_height, 50 - id as u64);
        }

        // Coordinate 50 pegouts (only 10 available). Still sorted by height.
        let pegouts = db.coord_pending_pegouts(50).unwrap();
        assert_eq!(pegouts.len(), 10);

        for i in 0..10 {
            let id = 9 - i;

            let pegout_id = PegoutId::new([id as u8; 32], 0);
            assert_eq!(pegouts[i].id, pegout_id);
            assert_eq!(pegouts[i].botanix_height, 50 - id as u64);
        }
    }

    #[test]
    fn should_store_many_pegouts() {
        let (db, _temp_dir) = setup_db();
        let num_pegouts = 5;
        let mut pegouts = vec![];
        for _ in 0..num_pegouts {
            let pegout_id = create_random_pegout_id();
            let req = pegout_scheduler::PegoutRequest {
                id: pegout_id,
                spk: random_p2wpkh_scriptpubkey(),
                value: Amount::from_sat(100_000),
                botanix_height: 1,
                timestamp: None,
            };
            pegouts.push(req);
        }
        let pegout_slice = pegouts.iter().collect::<Vec<&PegoutRequest>>();
        db.store_pending_pegouts(&pegout_slice).unwrap();
        db.flush().unwrap();

        // Get all pegouts
        let pegouts_retrieved = db.get_pending_pegouts().unwrap();
        assert_eq!(pegouts.len(), num_pegouts);
        // All pegouts should be present
        for pegout in pegouts.iter() {
            assert!(pegouts_retrieved.contains(pegout));
        }
    }

    // Should have the same outcome as should_store_many_pegouts
    #[test]
    fn should_store_many_pegouts_atomically() {
        let (db, _temp_dir) = setup_db();
        let num_pegouts = 5;
        let mut pegouts = vec![];
        for _ in 0..num_pegouts {
            let pegout_id = create_random_pegout_id();
            let req = pegout_scheduler::PegoutRequest {
                id: pegout_id,
                spk: random_p2wpkh_scriptpubkey(),
                value: Amount::from_sat(100_000),
                botanix_height: 1,
                timestamp: None,
            };
            pegouts.push(req);
        }
        let pegout_slice = pegouts.iter().collect::<Vec<&PegoutRequest>>();
        db.store_pending_pegouts_atomically(&pegout_slice).unwrap();
        db.flush().unwrap();

        // Get all pegouts
        let pegouts_retrieved = db.get_pending_pegouts().unwrap();
        assert_eq!(pegouts.len(), num_pegouts);
        // All pegouts should be present
        for pegout in pegouts.iter() {
            assert!(pegouts_retrieved.contains(pegout));
        }
    }

    #[test]
    fn can_remove_pending_pegout() {
        let (db, _temp_dir) = setup_db();

        // create 10 random pegouts
        for i in 0..10 {
            let pegout_id = PegoutId::new([i as u8; 32], 0);
            let req = pegout_scheduler::PegoutRequest {
                id: pegout_id,
                spk: random_p2wpkh_scriptpubkey(),
                value: Amount::from_sat(100_000),
                botanix_height: 1,
                timestamp: None,
            };
            db.store_pending_pegout(&req).unwrap();
        }
        let pegouts = db.get_pending_pegouts().unwrap();
        assert_eq!(pegouts.len(), 10);

        let first_pegout_id = pegouts.get(0).unwrap().id;

        db.remove_pending_pegout(&vec![first_pegout_id]).unwrap();
        let pegouts = db.get_pending_pegouts().unwrap();
        assert_eq!(pegouts.len(), 9);
    }

    #[test]
    fn utxo_rpc_conversion_round_trip() {
        // Test round-trip conversion: DbUtxo -> RpcUtxo -> DbUtxo
        use crate::test_utils::{random_compute_txid, random_p2tr_keyspend_script};

        let txid = random_compute_txid();

        // Test cases: (eth_address, utxo_version, description)
        let test_cases = [
            (Some([0; 20]), Some(UtxoVersion::V1), "with eth address and version"),
            (None, None, "without eth address or version"),
        ];

        for (vout, (eth_address, utxo_version, description)) in test_cases.iter().enumerate() {
            // Create a proper taproot TxOut instead of using create_tx which generates P2WPKH
            let taproot_output = TxOut {
                value: Amount::from_sat(1000),
                script_pubkey: random_p2tr_keyspend_script(),
            };

            let original_db_utxo = Utxo::new(
                OutPoint::new(txid, vout as u32),
                taproot_output,
                *eth_address,
                *utxo_version,
            );

            let rpc_utxo = RpcUtxo::try_from(original_db_utxo.clone()).unwrap();
            let recovered_utxo = Utxo::try_from(rpc_utxo).unwrap();

            assert_eq!(
                original_db_utxo, recovered_utxo,
                "Round-trip conversion should preserve all data ({})",
                description
            );
        }
    }

    #[test]
    fn utxo_rpc_conversion_with_script_len_prefix() {
        // Test to make sure we can still parse messages using the old RpcUtxo script format,
        // which included the length prefix. see https://github.com/botanix-labs/Macbeth/pull/949
        use bitcoin::consensus::encode::Encodable;

        // Create a taproot script using the existing helper
        let raw_script = crate::test_utils::random_p2tr_keyspend_script();

        // Create script bytes using the OLD way (consensus encoding with length prefix)
        let mut consensus_encoded_bytes = vec![];
        raw_script.consensus_encode(&mut consensus_encoded_bytes).unwrap();

        // Create a mock RpcUtxo with the consensus-encoded script bytes
        let rpc_utxo = RpcUtxo {
            outpoint: Some(RpcOutPoint {
                txid: vec![0; 32], // dummy txid
                vout: 0,
            }),
            output: Some(RpcTxOut {
                value: 1000,
                script_pubkey: Some(RpcScriptBuf {
                    script: consensus_encoded_bytes, // OLD format with length prefix
                }),
            }),
            eth_address: String::new(),
        };

        // Test that conversion from OLD format works successfully
        let recovered_utxo = Utxo::try_from(rpc_utxo).unwrap();
        assert_eq!(raw_script, recovered_utxo.output.script_pubkey);
    }

    #[test]
    fn test_parse_p2tr_script_with_fallback() {
        use bitcoin::consensus::encode::Encodable;

        let script = crate::test_utils::random_p2tr_keyspend_script();

        // Test raw bytes (current format)
        let raw_bytes = script.to_bytes();
        let parsed_raw = parse_p2tr_script_with_fallback(&raw_bytes);
        assert_eq!(script, parsed_raw);

        // Test consensus-encoded bytes (legacy format)
        let mut consensus_bytes = vec![];
        script.consensus_encode(&mut consensus_bytes).unwrap();
        let parsed_consensus = parse_p2tr_script_with_fallback(&consensus_bytes);
        assert_eq!(script, parsed_consensus);

        // Test invalid script bytes
        let invalid_bytes = vec![0x00, 0x14]; // Not a taproot script
        assert!(!parse_p2tr_script_with_fallback(&invalid_bytes).is_p2tr());
    }

    #[test]
    fn test_storing_single_utxo() {
        let (db, _temp_dir) = setup_db();

        let tx = create_tx(2, 1, None);
        let utxo = Utxo::new(
            OutPoint::new(tx.compute_txid(), 0),
            tx.output.get(0).expect("one output").clone(),
            None,
            None,
        );
        db.store_utxo(&utxo).unwrap();
        db.flush().unwrap();

        let retrieved_utxo = db.get_utxo(utxo.outpoint).unwrap().unwrap();
        assert!(retrieved_utxo == utxo);
    }

    #[test]
    fn test_storing_many_utxo() {
        let (db, _temp_dir) = setup_db();
        let num_txs = 5;
        let mut utxos = vec![];
        for _ in 0..num_txs {
            let tx = create_tx(2, 1, None);
            let utxo = Utxo::new(
                OutPoint::new(tx.compute_txid(), 0),
                tx.output.get(0).expect("one output").clone(),
                None,
                None,
            );
            utxos.push(utxo);
        }
        let utxo_slice = utxos.iter().collect::<Vec<&Utxo>>();
        db.store_utxos(&utxo_slice).unwrap();
        db.flush().unwrap();

        for utxo in utxos.iter() {
            let retrieved_utxo = db.get_utxo(utxo.outpoint).unwrap().unwrap();
            assert!(retrieved_utxo == *utxo);
        }

        // Get all utxos
        let retrieved_utxos = db.get_all_utxos().unwrap();
        println!("{:?}", retrieved_utxos);
        assert!(retrieved_utxos.len() == num_txs);
        // All utxos should be present
        for utxo in utxos.iter() {
            assert!(retrieved_utxos.contains(utxo));
        }
    }

    #[test]
    fn test_clear_utxos() {
        let (db, _temp_dir) = setup_db();
        let num_txs = 5;
        let mut utxos = vec![];
        for _ in 0..num_txs {
            let tx = create_tx(1, 1, None);
            let utxo = Utxo::new(
                OutPoint::new(tx.compute_txid(), 0),
                tx.output.get(0).expect("one output").clone(),
                None,
                None,
            );
            utxos.push(utxo);
        }
        let utxo_slice = utxos.iter().collect::<Vec<&Utxo>>();
        db.store_utxos(&utxo_slice).unwrap();
        db.flush().unwrap();

        db.clear_utxos().unwrap();
        db.flush().unwrap();
        // shouldn't have any utxos
        let retrieved_utxos = db.get_all_utxos().unwrap();
        assert!(retrieved_utxos.is_empty());
    }

    #[test]
    fn test_reset_utxos() {
        let (db, _temp_dir) = setup_db();
        let num_txs = 5;
        let mut utxos = vec![];
        for _ in 0..num_txs {
            let tx = create_tx(1, 1, None);
            let utxo = Utxo::new(
                OutPoint::new(tx.compute_txid(), 0),
                tx.output.get(0).expect("one output").clone(),
                None,
                None,
            );
            utxos.push(utxo);
        }
        let utxo_slice = utxos.iter().collect::<Vec<&Utxo>>();
        db.store_utxos(&utxo_slice).unwrap();
        db.flush().unwrap();

        let selected_utxos = utxos.iter().take(2).collect::<Vec<&Utxo>>();
        db.reset_utxos(&selected_utxos).unwrap();
        db.flush().unwrap();
        // shouldn't have any utxos
        let retrieved_utxos = db.get_all_utxos().unwrap();
        assert!(!retrieved_utxos.is_empty());
        assert!(retrieved_utxos.len() == 2);
        // Check the selected utxos are not in the set
        for utxo in selected_utxos.iter() {
            assert!(retrieved_utxos.contains(*utxo));
        }
    }

    // Should have the same outcome as test_storing_many_utxo
    #[test]
    fn test_storing_many_utxo_atomically() {
        let (db, _temp_dir) = setup_db();
        let num_txs = 5;
        let mut utxos = vec![];
        for _ in 0..num_txs {
            let tx = create_tx(2, 1, None);
            let utxo = Utxo::new(
                OutPoint::new(tx.compute_txid(), 0),
                tx.output.get(0).expect("one output").clone(),
                None,
                None,
            );
            utxos.push(utxo);
        }
        let utxo_slice = utxos.iter().collect::<Vec<&Utxo>>();
        db.store_utxos_atomically(&utxo_slice).unwrap();
        db.flush().unwrap();

        for utxo in utxos.iter() {
            let retrieved_utxo = db.get_utxo(utxo.outpoint).unwrap().unwrap();
            assert!(retrieved_utxo == *utxo);
        }

        // Get all utxos
        let retrieved_utxos = db.get_all_utxos().unwrap();
        println!("{:?}", retrieved_utxos);
        assert!(retrieved_utxos.len() == num_txs);
        // All utxos should be present
        for utxo in utxos.iter() {
            assert!(retrieved_utxos.contains(utxo));
        }
    }

    #[test]
    fn test_update_utxo_merkle_root() {
        let (db, _temp_dir) = setup_db();
        let num_txs = 5;
        let mut utxos = vec![];
        for _ in 0..num_txs {
            let tx = create_tx(2, 1, None);
            let utxo = Utxo::new(
                OutPoint::new(tx.compute_txid(), 0),
                tx.output.get(0).expect("one output").clone(),
                None,
                None,
            );
            utxos.push(utxo);
        }
        let utxo_slice = utxos.iter().collect::<Vec<&Utxo>>();
        db.store_utxos(&utxo_slice).unwrap();
        db.update_utxo_merkle_root().unwrap();
        db.flush().unwrap();

        let merkle_root = db.get_utxo_merkle_root().unwrap().unwrap();
        // Updating again should not change the merkle root
        db.update_utxo_merkle_root().unwrap();
        db.flush().unwrap();
        let merkle_root2 = db.get_utxo_merkle_root().unwrap().unwrap();
        assert_eq!(merkle_root, merkle_root2);

        // Adding an additional utxo should change the merkle root
        let tx = create_tx(2, 1, None);
        let utxo = Utxo::new(
            OutPoint::new(tx.compute_txid(), 1),
            tx.output.get(0).expect("one output").clone(),
            None,
            None,
        );
        db.store_utxo(&utxo).unwrap();
        db.update_utxo_merkle_root().unwrap();
        db.flush().unwrap();
        let merkle_root3 = db.get_utxo_merkle_root().unwrap().unwrap();
        assert_ne!(merkle_root, merkle_root3);
    }

    #[test]
    fn test_update_finalized_pegout_ids_merkle_root() {
        let (db, _temp_dir) = setup_db();
        let num_txs = 5;
        let mut finalized_pegout_ids = vec![];
        let mut rng = thread_rng();
        for i in 0..num_txs {
            let pegout_id = PegoutId::new(rng.gen::<[u8; 32]>(), i as u32);
            let finalized_pegout =
                FinalizedPegout { id: pegout_id, block_number: 100, timestamp: None };
            finalized_pegout_ids.push(finalized_pegout);
        }
        let finalized_pegout_ids_slice =
            finalized_pegout_ids.iter().collect::<Vec<&FinalizedPegout>>();
        db.store_finalized_pegout_ids(&finalized_pegout_ids_slice).unwrap();
        db.update_finalized_pegout_ids_merkle_root().unwrap();
        db.flush().unwrap();

        let merkle_root = db.get_finalized_pegout_ids_merkle_root().unwrap().unwrap();
        // Updating again should not change the merkle root
        db.update_finalized_pegout_ids_merkle_root().unwrap();
        db.flush().unwrap();
        let merkle_root2 = db.get_finalized_pegout_ids_merkle_root().unwrap().unwrap();
        assert_eq!(merkle_root, merkle_root2);

        // // Adding an additional pegout id should change the merkle root
        let pegout_id = PegoutId::new(rng.gen::<[u8; 32]>(), num_txs + 1 as u32);
        let finalized_pegout =
            FinalizedPegout { id: pegout_id, block_number: 100, timestamp: None };
        db.store_finalized_pegout_id(&finalized_pegout).unwrap();
        db.update_finalized_pegout_ids_merkle_root().unwrap();
        db.flush().unwrap();
        let merkle_root3 = db.get_finalized_pegout_ids_merkle_root().unwrap().unwrap();
        assert_ne!(merkle_root, merkle_root3);
    }

    #[test]
    fn test_prune_finalized_pegouts_ids() {
        let (db, _temp_dir) = setup_db();
        let num_txs = 3;
        let mut finalized_pegout_ids = vec![];
        let mut rng = thread_rng();
        for i in 0..num_txs {
            let pegout_id = PegoutId::new(rng.gen::<[u8; 32]>(), i as u32);
            let finalized_pegout =
                FinalizedPegout { id: pegout_id, block_number: 100, timestamp: None };
            finalized_pegout_ids.push(finalized_pegout);
        }

        // update finalized pegout to be within the pruning window
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        finalized_pegout_ids[0].timestamp = Some(now);

        // update finalized pegout to be outside the pruning window
        finalized_pegout_ids[1].timestamp = Some(now.saturating_sub(RETENTION_WINDOW_SECONDS + 1));

        let finalized_pegout_ids_slice =
            finalized_pegout_ids.iter().collect::<Vec<&FinalizedPegout>>();

        // We now have 3 finalized pegouts in the following order:
        // - one with a timestamp within the pruning window
        // - one with a timestamp outside the pruning window
        // - one without a timestamp (None)

        db.store_finalized_pegout_ids(&finalized_pegout_ids_slice).unwrap();
        db.flush().unwrap();

        db.prune_finalized_pegout_ids().unwrap();
        db.flush().unwrap();

        let retrieved_pegouts = db.get_finalized_pegout_ids().unwrap();
        // There should be 2 finalized pegouts left: finalized_pegout_ids[0] and
        // finalized_pegout_ids[2] finalized_pegout_ids[1] should be pruned since it has a
        // timestamp outside the pruning window
        assert_eq!(retrieved_pegouts.len(), 2);

        // Check that the pegout with a timestamp within the pruning window is still present
        let pegout_with_timestamp = retrieved_pegouts
            .iter()
            .find(|p| p.id == finalized_pegout_ids[0].id)
            .expect("Pegout with timestamp should still be present");
        assert_eq!(pegout_with_timestamp.timestamp, finalized_pegout_ids[0].timestamp);

        // Check that the pegout without a timestamp is still present
        let pegout_without_timestamp = retrieved_pegouts
            .iter()
            .find(|p| p.id == finalized_pegout_ids[2].id)
            .expect("Pegout without timestamp should still be present");
        assert!(pegout_without_timestamp.timestamp.is_some());
    }

    #[tokio::test]
    async fn test_stream_pegout_ids_chunksize_lt_items() {
        let (db, _temp_dir) = setup_db();
        let num_txs = 52;
        let mut finalized_pegout_ids = vec![];
        let mut rng = thread_rng();
        for i in 0..num_txs {
            let pegout_id = PegoutId::new(rng.gen::<[u8; 32]>(), i as u32);
            let finalized_pegout =
                FinalizedPegout { id: pegout_id, block_number: 100, timestamp: None };
            finalized_pegout_ids.push(finalized_pegout);
        }
        let finalized_pegout_ids_slice =
            finalized_pegout_ids.iter().collect::<Vec<&FinalizedPegout>>();
        db.store_finalized_pegout_ids(&finalized_pegout_ids_slice).unwrap();
        db.flush().unwrap();

        let chunk_size = 10;
        let stream = db.get_finalized_pegout_ids_stream(chunk_size);
        pin!(stream);
        let mut total_count = 0;
        let expected_total_chunks = (num_txs as u64).div_ceil(chunk_size as u64);
        let mut chunk_indexes_set = HashSet::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok((pegout_ids, chunk_index, num_chunks)) => {
                    chunk_indexes_set.insert(chunk_index);
                    assert_eq!(num_chunks, expected_total_chunks);
                    total_count += pegout_ids.len();
                }
                Err(e) => panic!("Error streaming pegout ids: {:?}", e),
            }
        }
        assert_eq!(total_count as u64, num_txs);
        assert_eq!(
            chunk_indexes_set.len() as u64,
            (total_count as u64).div_ceil(chunk_size as u64)
        );
    }

    #[tokio::test]
    async fn test_stream_pegout_ids_chunksize_gt_items() {
        let (db, _temp_dir) = setup_db();
        let num_txs = 2;
        let mut finalized_pegout_ids = vec![];
        let mut rng = thread_rng();
        for i in 0..num_txs {
            let pegout_id = PegoutId::new(rng.gen::<[u8; 32]>(), i as u32);
            let finalized_pegout =
                FinalizedPegout { id: pegout_id, block_number: 100, timestamp: None };
            finalized_pegout_ids.push(finalized_pegout);
        }
        let finalized_pegout_ids_slice =
            finalized_pegout_ids.iter().collect::<Vec<&FinalizedPegout>>();
        db.store_finalized_pegout_ids(&finalized_pegout_ids_slice).unwrap();
        db.flush().unwrap();

        let chunk_size = 10;
        let stream = db.get_finalized_pegout_ids_stream(chunk_size);
        pin!(stream);
        let mut total_count = 0;
        let expected_total_chunks = (num_txs as u64).div_ceil(chunk_size as u64);
        let mut chunk_indexes_set = HashSet::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok((pegout_ids, chunk_index, num_chunks)) => {
                    chunk_indexes_set.insert(chunk_index);
                    assert_eq!(num_chunks, expected_total_chunks);
                    total_count += pegout_ids.len();
                }
                Err(e) => panic!("Error streaming pegout ids: {:?}", e),
            }
        }
        assert_eq!(total_count as u64, num_txs);
        assert_eq!(
            chunk_indexes_set.len() as u64,
            (total_count as u64).div_ceil(chunk_size as u64)
        );
    }

    #[test]
    fn should_store_many_finalized_pegout_ids_atomically() {
        let (db, _temp_dir) = setup_db();
        let num_pegout_ids = 5;
        let mut pegouts = vec![];
        for _ in 0..num_pegout_ids {
            let pegout_id = create_random_pegout_id();
            let finalized_pegout =
                FinalizedPegout { id: pegout_id, block_number: 100, timestamp: None };
            pegouts.push(finalized_pegout);
        }
        let pegout_slice = pegouts.iter().collect::<Vec<&FinalizedPegout>>();
        db.store_finalized_pegout_ids_atomically(&pegout_slice).unwrap();
        db.flush().unwrap();

        // Get all pegout ids
        let pegouts_retrieved = db.get_finalized_pegout_ids().unwrap();
        assert_eq!(pegouts.len(), num_pegout_ids);
        // All pegouts should be present
        for pegout in pegouts.iter() {
            assert!(pegouts_retrieved.contains(pegout));
        }
    }

    #[test]
    fn should_store_many_finalized_pegout_ids() {
        let (db, _temp_dir) = setup_db();
        let num_pegout_ids = 5;
        let mut pegouts = vec![];
        for _ in 0..num_pegout_ids {
            let pegout_id = create_random_pegout_id();
            let finalized_pegout =
                FinalizedPegout { id: pegout_id, block_number: 100, timestamp: None };
            pegouts.push(finalized_pegout);
        }
        let pegout_slice = pegouts.iter().collect::<Vec<&FinalizedPegout>>();
        db.store_finalized_pegout_ids(&pegout_slice).unwrap();
        db.flush().unwrap();

        // Get all pegout ids
        let pegouts_retrieved = db.get_finalized_pegout_ids().unwrap();
        assert_eq!(pegouts.len(), num_pegout_ids);
        // All pegouts should be present
        for pegout in pegouts.iter() {
            assert!(pegouts_retrieved.contains(pegout));
        }
    }

    #[test]
    fn test_reading_session_ids() {
        let (db, _temp_dir) = setup_db();

        let tx = create_tx(2, 1, None);
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

        let tx = create_tx(2, 1, None);
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

    #[test]
    fn test_tracked_txs_e2e() {
        let (db, _temp_dir) = setup_db();
        let tx = create_tx(5, 2, None);
        let pegout_reqs = vec![PegoutRequest {
            spk: tx.output[0].script_pubkey.clone(),
            value: tx.output[0].value,
            id: create_random_pegout_id(),
            botanix_height: 0,
            timestamp: None,
        }];
        let tracked_tx = Tx {
            txid: tx.compute_txid(),
            tx: tx.clone(),
            change_idxs: vec![1],
            pegout_idxs: vec![0],
            pegout_requests: pegout_reqs,
            created: SystemTime::now(),
        };
        db.store_tracked_tx(&tracked_tx).unwrap();
        db.flush().unwrap();

        let tx_retrieved = db.get_tracked_txs().unwrap();
        assert_eq!(tx_retrieved[0], tracked_tx);

        // Storing the same tx again should not add a new entry
        db.store_tracked_tx(&tracked_tx).unwrap();
        db.flush().unwrap();
        let tx_retrieved = db.get_tracked_txs().unwrap();
        assert_eq!(tx_retrieved[0], tracked_tx);

        // Remove the tracked tx
        db.remove_tracked_tx(&tx.compute_txid()).unwrap();
        db.flush().unwrap();
        let tx_retrieved = db.get_tracked_txs().unwrap();
        assert_eq!(tx_retrieved.len(), 0);
    }

    #[test]
    fn should_store_many_tracked_txs() {
        let (db, _temp_dir) = setup_db();
        let num_txs = 5;
        let mut txs = vec![];
        for _ in 0..num_txs {
            let tx = create_tx(5, 2, None);
            let pegout_reqs = vec![PegoutRequest {
                spk: tx.output[0].script_pubkey.clone(),
                value: tx.output[0].value,
                id: create_random_pegout_id(),
                botanix_height: 0,
                timestamp: None,
            }];
            let tracked_tx = Tx {
                txid: tx.compute_txid(),
                tx: tx.clone(),
                change_idxs: vec![1],
                pegout_idxs: vec![0],
                pegout_requests: pegout_reqs,
                created: SystemTime::now(),
            };
            txs.push(tracked_tx);
        }
        let tx_slice = txs.iter().collect::<Vec<&Tx>>();
        db.store_tracked_txs(&tx_slice).unwrap();
        db.flush().unwrap();

        // Get all tracked txs
        let txs_retrieved = db.get_tracked_txs().unwrap();
        assert_eq!(txs_retrieved.len(), num_txs);
        // All txs should be present
        for tx in txs.iter() {
            assert!(txs_retrieved.contains(tx));
        }
    }

    // Should have the same outcome as test_should_store_many_tracked_txs
    #[test]
    fn should_store_many_tracked_txs_atomically() {
        let (db, _temp_dir) = setup_db();
        let num_txs = 5;
        let mut txs = vec![];
        for _ in 0..num_txs {
            let tx = create_tx(5, 2, None);
            let pegout_reqs = vec![PegoutRequest {
                spk: tx.output[0].script_pubkey.clone(),
                value: tx.output[0].value,
                id: create_random_pegout_id(),
                botanix_height: 0,
                timestamp: None,
            }];
            let tracked_tx = Tx {
                txid: tx.compute_txid(),
                tx: tx.clone(),
                change_idxs: vec![1],
                pegout_idxs: vec![0],
                pegout_requests: pegout_reqs,
                created: SystemTime::now(),
            };
            txs.push(tracked_tx);
        }
        let tx_slice = txs.iter().collect::<Vec<&Tx>>();
        db.store_tracked_txs_atomically(&tx_slice).unwrap();
        db.flush().unwrap();

        // Get all tracked txs
        let txs_retrieved = db.get_tracked_txs().unwrap();
        assert_eq!(txs_retrieved.len(), num_txs);
        // All txs should be present
        for tx in txs.iter() {
            assert!(txs_retrieved.contains(tx));
        }
    }

    #[test]
    fn test_update_tracked_tx_merkle_root() {
        let (db, _temp_dir) = setup_db();
        let tx = create_tx(5, 2, None);
        let pegout_reqs = vec![PegoutRequest {
            spk: tx.output[0].script_pubkey.clone(),
            value: tx.output[0].value,
            id: create_random_pegout_id(),
            botanix_height: 0,
            timestamp: None,
        }];
        let tracked_tx = Tx {
            txid: tx.compute_txid(),
            tx: tx.clone(),
            change_idxs: vec![1],
            pegout_idxs: vec![0],
            pegout_requests: pegout_reqs,
            created: SystemTime::now(),
        };
        db.store_tracked_tx(&tracked_tx).unwrap();
        db.flush().unwrap();
        db.update_tracked_tx_merkle_root().unwrap();
        db.flush().unwrap();

        let merkle_root = db.get_tracked_tx_merkle_root().unwrap().unwrap();
        db.update_tracked_tx_merkle_root().unwrap();
        db.flush().unwrap();
        let merkle_root2 = db.get_tracked_tx_merkle_root().unwrap().unwrap();
        assert_eq!(merkle_root, merkle_root2);

        let tx2 = create_tx(5, 2, None);
        let pegout_reqs = vec![PegoutRequest {
            spk: tx2.output[0].script_pubkey.clone(),
            value: tx.output[0].value,
            id: create_random_pegout_id(),
            botanix_height: 0,
            timestamp: None,
        }];
        let tracked_tx2 = Tx {
            txid: tx2.compute_txid(),
            tx: tx2.clone(),
            change_idxs: vec![1],
            pegout_idxs: vec![0],
            pegout_requests: pegout_reqs,
            created: SystemTime::now(),
        };
        db.store_tracked_tx(&tracked_tx2).unwrap();
        db.update_tracked_tx_merkle_root().unwrap();
        db.flush().unwrap();

        let merkle_root3 = db.get_tracked_tx_merkle_root().unwrap().unwrap();
        assert_ne!(merkle_root, merkle_root3);
    }

    #[test]
    fn test_update_pending_pegouts_merkle_root() {
        let (db, _temp_dir) = setup_db();
        db.update_pending_pegouts_merkle_root().unwrap();
        db.flush().unwrap();
        let merkle_root = db.get_pending_pegouts_merkle_root().unwrap();

        assert!(merkle_root.is_none());

        let tx = create_tx(5, 2, None);

        let pegout_req = PegoutRequest {
            botanix_height: 0,
            id: create_random_pegout_id(),
            spk: tx.output[0].script_pubkey.clone(),
            value: tx.output[0].value,
            timestamp: None,
        };
        db.store_pending_pegout(&pegout_req).unwrap();
        db.flush().unwrap();

        let merkle_root = db.get_pending_pegouts_merkle_root().unwrap().unwrap();

        // Assert the same pending pegout added again does not change the merkle root
        db.store_pending_pegout(&pegout_req).unwrap();
        db.flush().unwrap();
        let merkle_root2 = db.get_pending_pegouts_merkle_root().unwrap().unwrap();
        assert_eq!(merkle_root, merkle_root2);

        // Add a second pending pegout
        let pegout_req2 = PegoutRequest {
            botanix_height: 0,
            id: create_random_pegout_id(),
            spk: tx.output[1].script_pubkey.clone(),
            value: tx.output[1].value,
            timestamp: None,
        };
        db.store_pending_pegout(&pegout_req2).unwrap();
        db.flush().unwrap();

        let merkle_root3 = db.get_pending_pegouts_merkle_root().unwrap().unwrap();
        assert_ne!(merkle_root, merkle_root3);
    }

    #[test]
    fn clear_pending_pegouts_should_clear_db() {
        let (db, _temp_dir) = setup_db();
        let tx = create_tx(5, 2, None);

        let pegout_req = PegoutRequest {
            botanix_height: 0,
            id: create_random_pegout_id(),
            spk: tx.output[0].script_pubkey.clone(),
            value: tx.output[0].value,
            timestamp: None,
        };
        db.store_pending_pegout(&pegout_req).unwrap();
        db.flush().unwrap();

        db.clear_pending_pegouts().unwrap();
        db.flush().unwrap();

        let pending_pegouts = db.get_pending_pegouts().unwrap();
        assert!(pending_pegouts.is_empty());
    }

    #[test]
    fn reset_pending_pegouts_should_clear_db_and_readd() {
        let (db, _temp_dir) = setup_db();
        let tx = create_tx(5, 2, None);

        let pegout_req = PegoutRequest {
            botanix_height: 0,
            id: create_random_pegout_id(),
            spk: tx.output[0].script_pubkey.clone(),
            value: tx.output[0].value,
            timestamp: None,
        };
        db.store_pending_pegout(&pegout_req).unwrap();
        db.flush().unwrap();

        let tx2 = create_tx(5, 2, None);
        let pegout_req2 = PegoutRequest {
            botanix_height: 0,
            id: create_random_pegout_id(),
            spk: tx2.output[0].script_pubkey.clone(),
            value: tx2.output[0].value,
            timestamp: None,
        };
        db.reset_pending_pegouts(&[&pegout_req2]).unwrap();
        db.flush().unwrap();

        let pending_pegouts = db.get_pending_pegouts().unwrap();
        assert_eq!(pending_pegouts.len(), 1);
        assert_eq!(pending_pegouts[0], pegout_req2);
    }

    #[test]
    fn clear_tracked_txs_should_clear_db() {
        let (db, _temp_dir) = setup_db();
        let tx = create_tx(5, 2, None);
        let pegout_requests = vec![PegoutRequest {
            spk: tx.output[0].script_pubkey.clone(),
            value: tx.output[0].value,
            id: create_random_pegout_id(),
            botanix_height: 0,
            timestamp: None,
        }];
        let tracked_tx = Tx {
            txid: tx.compute_txid(),
            tx: tx.clone(),
            change_idxs: vec![1],
            pegout_idxs: vec![0],
            pegout_requests,
            created: SystemTime::now(),
        };
        db.store_tracked_tx(&tracked_tx).unwrap();
        db.flush().unwrap();

        db.clear_tracked_txs().unwrap();
        db.flush().unwrap();

        let tracked_txs = db.get_tracked_txs().unwrap();
        assert!(tracked_txs.is_empty());
    }

    #[test]
    fn reset_tracked_txs_should_clear_db_and_readd() {
        let (db, _temp_dir) = setup_db();
        let tx = create_tx(5, 2, None);
        let pegout_requests = vec![PegoutRequest {
            spk: tx.output[0].script_pubkey.clone(),
            value: tx.output[0].value,
            id: create_random_pegout_id(),
            botanix_height: 0,
            timestamp: None,
        }];
        let tracked_tx = Tx {
            txid: tx.compute_txid(),
            tx: tx.clone(),
            change_idxs: vec![1],
            pegout_idxs: vec![0],
            pegout_requests,
            created: SystemTime::now(),
        };
        db.store_tracked_tx(&tracked_tx).unwrap();
        db.flush().unwrap();

        let tx2 = create_tx(5, 2, None);
        let pegout_requests2 = vec![PegoutRequest {
            spk: tx2.output[0].script_pubkey.clone(),
            value: tx2.output[0].value,
            id: create_random_pegout_id(),
            botanix_height: 0,
            timestamp: None,
        }];
        let tracked_tx2 = Tx {
            txid: tx2.compute_txid(),
            tx: tx2.clone(),
            change_idxs: vec![1],
            pegout_idxs: vec![0],
            pegout_requests: pegout_requests2,
            created: SystemTime::now(),
        };
        db.reset_tracked_txs(&[&tracked_tx2]).unwrap();
        db.flush().unwrap();

        let tracked_txs = db.get_tracked_txs().unwrap();
        assert_eq!(tracked_txs.len(), 1);
        assert_eq!(tracked_txs[0], tracked_tx2);
    }

    #[test]
    fn test_deserialize_old_data_with_json() {
        let mut rng = thread_rng();
        // Simulate old serialized data (without timestamp field)
        let pegout_id = PegoutId::new(rng.gen::<[u8; 32]>(), 1 as u32);
        let old_pegout = OldFinalizedPegout { id: pegout_id.clone(), block_number: 100 };

        // Serialize with old structure
        let serialized_old = serde_json::to_vec(&old_pegout).unwrap();

        // Deserialize with new structure - should have timestamp = None
        let deserialized_new: FinalizedPegout = serde_json::from_slice(&serialized_old).unwrap();

        assert_eq!(deserialized_new.id, pegout_id);
        assert_eq!(deserialized_new.block_number, 100);
        assert_eq!(deserialized_new.timestamp, None);
    }

    #[test]
    fn test_deserialize_new_data_with_some_timestamp() {
        let mut rng = thread_rng();
        // Simulate new serialized data (with timestamp field)
        let pegout_id = PegoutId::new(rng.gen::<[u8; 32]>(), 1 as u32);
        // Test new data with explicit Some timestamp
        let new_pegout = FinalizedPegout {
            id: pegout_id.clone(),
            block_number: 200,
            timestamp: Some(1234567890), // Example timestamp
        };

        // Serialize and deserialize to old finalized pegout
        let serialized = serde_json::to_vec(&new_pegout).unwrap();
        let deserialized: OldFinalizedPegout = serde_json::from_slice(&serialized).unwrap();

        assert_eq!(deserialized.id, pegout_id);
        assert_eq!(deserialized.block_number, 200);
    }

    #[tokio::test]
    async fn test_export_import_key_package() {
        let (db, _temp_dir) = setup_db();

        let good_pass = Zeroizing::new("good_pass".to_string());
        let bad_pass = Zeroizing::new("bad_pass".to_string());

        // Key package does not exist yet.
        let res = db.export_key_package(good_pass.clone()).unwrap();
        assert!(res.is_none());

        // Generate key packages.
        let id = frost::Identifier::derive(0_u16.to_le_bytes().as_slice()).unwrap();
        let (shares, pk_package) = trusted_dealer_setup(2, 3);
        let key_package = frost::keys::KeyPackage::try_from(shares[&id].clone()).unwrap();

        // Set key packages.
        db.set_pubkey_package(pk_package.clone()).unwrap();
        db.set_key_package(key_package.clone()).unwrap();

        let origin_pk_package = pk_package;
        let origin_key_package = key_package;

        // Each export creates a new nonce.
        let mut export_1 = db.export_key_package(good_pass.clone()).unwrap().unwrap();
        let export_2 = db.export_key_package(good_pass.clone()).unwrap().unwrap();
        //
        assert_ne!(export_1.iv, export_2.iv);
        assert_ne!(export_1, export_2);

        let prev_key = db.db.remove(TREE_KEY_PACKAGE).unwrap();
        let prev_pk = db.db.remove(TREE_PUBKEY_PACKAGE).unwrap();
        assert!(prev_key.is_some());
        assert!(prev_pk.is_some());

        // ERR: Bad password!
        let err = db.import_key_package(bad_pass, export_1.clone()).unwrap_err();
        assert_eq!(err, Error::BadDecryptionPassphrase);

        // ERR: Bad IV/nonce!
        export_1.iv = export_2.iv;
        let err = db.import_key_package(good_pass.clone(), export_1.clone()).unwrap_err();
        assert_eq!(err, Error::BadDecryptionPassphrase);

        // ERR: Bad version indicator!
        export_1.version = u16::MAX;
        let err = db.import_key_package(good_pass.clone(), export_1.clone()).unwrap_err();
        assert_eq!(err, Error::BadExportedPackageFormatVersion);

        // OK: Successful import with good passphrase and export package.
        db.import_key_package(good_pass.clone(), export_2.clone()).unwrap();

        // Sanity check.
        let new_pk_package = db.get_public_key_package().unwrap().unwrap();
        let new_key_package = db.get_key_package().unwrap().unwrap();
        //
        assert_eq!(new_pk_package, origin_pk_package);
        assert_eq!(new_key_package, origin_key_package);
    }

    #[test]
    fn test_set_and_get_key_package_by_id() {
        let (db, _temp_dir) = setup_db();

        // Generate key packages for multiple multisigs
        let id = frost::Identifier::derive(0_u16.to_le_bytes().as_slice()).unwrap();
        let (shares, pk_package) = trusted_dealer_setup(2, 3);
        let key_package = frost::keys::KeyPackage::try_from(shares[&id].clone()).unwrap();

        // Store with multisig_id = 0
        db.set_key_package_by_id(0, key_package.clone()).unwrap();
        db.set_pubkey_package_by_id(0, pk_package.clone()).unwrap();

        // Retrieve and verify
        let retrieved_key = db.get_key_package_by_id(0).unwrap().unwrap();
        let retrieved_pk = db.get_public_key_package_by_id(0).unwrap().unwrap();

        assert_eq!(retrieved_key, key_package);
        assert_eq!(retrieved_pk, pk_package);

        // Non-existent multisig_id should return None
        assert!(db.get_key_package_by_id(999).unwrap().is_none());
        assert!(db.get_public_key_package_by_id(999).unwrap().is_none());
    }

    #[test]
    fn test_multiple_multisig_ids() {
        let (db, _temp_dir) = setup_db();

        // Generate key packages for multiple multisigs
        let id = frost::Identifier::derive(0_u16.to_le_bytes().as_slice()).unwrap();
        let (shares1, pk_package1) = trusted_dealer_setup(2, 3);
        let key_package1 = frost::keys::KeyPackage::try_from(shares1[&id].clone()).unwrap();

        let (shares2, pk_package2) = trusted_dealer_setup(3, 5);
        let key_package2 = frost::keys::KeyPackage::try_from(shares2[&id].clone()).unwrap();

        // Store with different multisig_ids
        db.set_key_package_by_id(1, key_package1.clone()).unwrap();
        db.set_pubkey_package_by_id(1, pk_package1.clone()).unwrap();

        db.set_key_package_by_id(2, key_package2.clone()).unwrap();
        db.set_pubkey_package_by_id(2, pk_package2.clone()).unwrap();

        // Retrieve and verify both are correct
        let retrieved_key1 = db.get_key_package_by_id(1).unwrap().unwrap();
        let retrieved_pk1 = db.get_public_key_package_by_id(1).unwrap().unwrap();
        assert_eq!(retrieved_key1, key_package1);
        assert_eq!(retrieved_pk1, pk_package1);

        let retrieved_key2 = db.get_key_package_by_id(2).unwrap().unwrap();
        let retrieved_pk2 = db.get_public_key_package_by_id(2).unwrap().unwrap();
        assert_eq!(retrieved_key2, key_package2);
        assert_eq!(retrieved_pk2, pk_package2);
    }

    #[test]
    fn test_list_multisig_ids() {
        let (db, _temp_dir) = setup_db();

        // Empty list initially
        let ids = db.list_multisig_ids().unwrap();
        assert!(ids.is_empty());

        // Generate and store key packages with different multisig_ids
        let id = frost::Identifier::derive(0_u16.to_le_bytes().as_slice()).unwrap();
        let (shares, _pk_package) = trusted_dealer_setup(2, 3);
        let key_package = frost::keys::KeyPackage::try_from(shares[&id].clone()).unwrap();

        // Add in non-sequential order
        db.set_key_package_by_id(10, key_package.clone()).unwrap();
        db.set_key_package_by_id(5, key_package.clone()).unwrap();
        db.set_key_package_by_id(1, key_package.clone()).unwrap();

        // List should be sorted
        let ids = db.list_multisig_ids().unwrap();
        assert_eq!(ids, vec![1, 5, 10]);
    }

    #[test]
    fn test_old_and_new_methods_dont_interfere() {
        let (db, _temp_dir) = setup_db();

        // Generate key packages
        let id = frost::Identifier::derive(0_u16.to_le_bytes().as_slice()).unwrap();
        let (shares1, pk_package1) = trusted_dealer_setup(2, 3);
        let key_package1 = frost::keys::KeyPackage::try_from(shares1[&id].clone()).unwrap();

        let (shares2, pk_package2) = trusted_dealer_setup(3, 5);
        let key_package2 = frost::keys::KeyPackage::try_from(shares2[&id].clone()).unwrap();

        // Store using old methods (single key)
        db.set_key_package(key_package1.clone()).unwrap();
        db.set_pubkey_package(pk_package1.clone()).unwrap();

        // Store using new methods (multi-key)
        db.set_key_package_by_id(0, key_package2.clone()).unwrap();
        db.set_pubkey_package_by_id(0, pk_package2.clone()).unwrap();

        // Old methods should still return the old data
        let old_key = db.get_key_package().unwrap().unwrap();
        let old_pk = db.get_public_key_package().unwrap().unwrap();
        assert_eq!(old_key, key_package1);
        assert_eq!(old_pk, pk_package1);

        // New methods should return the new data
        let new_key = db.get_key_package_by_id(0).unwrap().unwrap();
        let new_pk = db.get_public_key_package_by_id(0).unwrap().unwrap();
        assert_eq!(new_key, key_package2);
        assert_eq!(new_pk, pk_package2);

        // They should be different
        assert_ne!(old_key, new_key);
        assert_ne!(old_pk, new_pk);
    }

    #[test]
    fn test_migrate_legacy_key_package() {
        let (db, _temp_dir) = setup_db();

        // Generate key packages
        let id = frost::Identifier::derive(0_u16.to_le_bytes().as_slice()).unwrap();
        let (shares, pk_package) = trusted_dealer_setup(2, 3);
        let key_package = frost::keys::KeyPackage::try_from(shares[&id].clone()).unwrap();

        // Store using old methods (simulating legacy production data)
        db.set_key_package(key_package.clone()).unwrap();
        db.set_pubkey_package(pk_package.clone()).unwrap();

        // Verify old storage has data
        assert!(db.get_key_package().unwrap().is_some());
        assert!(db.get_public_key_package().unwrap().is_some());

        // Verify new storage is empty
        assert!(db.get_key_package_by_id(0).unwrap().is_none());
        assert!(db.get_public_key_package_by_id(0).unwrap().is_none());

        // Run migration
        let migrated = db.migrate_legacy_key_package().unwrap();
        assert!(migrated, "Migration should have been performed");

        // Verify new storage now has the data at multisig_id = 0
        let migrated_key = db.get_key_package_by_id(0).unwrap().unwrap();
        let migrated_pk = db.get_public_key_package_by_id(0).unwrap().unwrap();
        assert_eq!(migrated_key, key_package);
        assert_eq!(migrated_pk, pk_package);

        // Old storage should still have data (non-destructive migration)
        assert!(db.get_key_package().unwrap().is_some());
        assert!(db.get_public_key_package().unwrap().is_some());

        // Running migration again should skip (idempotent)
        let migrated_again = db.migrate_legacy_key_package().unwrap();
        assert!(!migrated_again, "Second migration should be skipped");
    }

    #[test]
    fn test_migrate_legacy_key_package_no_legacy_data() {
        let (db, _temp_dir) = setup_db();

        // No legacy data, no new data - migration should be skipped
        let migrated = db.migrate_legacy_key_package().unwrap();
        assert!(!migrated, "Migration should be skipped when no legacy data exists");
    }

    #[test]
    fn test_migrate_legacy_key_package_incomplete_legacy_data() {
        let (db, _temp_dir) = setup_db();

        // Generate only key package, not public key (incomplete legacy data)
        let id = frost::Identifier::derive(0_u16.to_le_bytes().as_slice()).unwrap();
        let (shares, _pk_package) = trusted_dealer_setup(2, 3);
        let key_package = frost::keys::KeyPackage::try_from(shares[&id].clone()).unwrap();

        // Store only key package (incomplete)
        db.set_key_package(key_package).unwrap();

        // Migration should be skipped due to incomplete data
        let migrated = db.migrate_legacy_key_package().unwrap();
        assert!(!migrated, "Migration should be skipped when legacy data is incomplete");
    }
}
