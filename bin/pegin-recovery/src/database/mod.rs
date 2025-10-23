use std::{collections::BTreeMap, path::Path};

#[allow(unused_imports)]
use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit, Nonce};
use frost_secp256k1_tr as frost;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub mod error;
pub use error::Error;

/// Version number for the exported package format.
pub const EXPORTED_PACKAGE_VERSION: u16 = 0;

/// Tree identifier for key shares storage.
const TREE_KEY_SHARES: &[u8; 9] = b"keyshares";

/// Represents a collection of key shares for a single multisig.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MultisigKeyShares {
    /// Map of node_id -> FROST key package
    pub shares: BTreeMap<Vec<u8>, frost::keys::KeyPackage>,
    /// Optional: metadata like creation time, version, etc.
    pub metadata: Option<MultisigMetadata>,
}

/// Optional metadata for a multisig's key shares.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MultisigMetadata {
    pub created_at: u64,
    pub description: Option<String>,
}

/// Encrypted export format for key shares.
/// Follows the same pattern as btc-server's ExportedKeyPackage.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ExportedKeyShares {
    /// Version indicator (future-reserved), currently it's
    /// [`EXPORTED_PACKAGE_VERSION`].
    pub version: u16,
    /// Random 96-bit nonce used for encryption operations, in plaintext.
    pub iv: [u8; 12],
    /// Encrypted key shares data. Contains the encrypted key material
    /// with authentication tag.
    pub enc_key_shares: Vec<u8>,
}

/// Database handle for the pegin-recovery service.
#[derive(Clone)]
pub struct Db {
    /// The underlying sled database.
    db: sled::Db,
    /// Tree for storing key shares, indexed by multisig_id.
    key_shares: sled::Tree,
}

impl Db {
    /// Opens or creates a database at the specified path.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the database directory.
    ///
    /// # Returns
    ///
    /// Returns a `Db` instance if successful, or a sled error.
    pub fn open(path: impl AsRef<Path>) -> Result<Db, sled::Error> {
        let db = sled::open(path)?;
        Ok(Db { key_shares: db.open_tree(TREE_KEY_SHARES)?, db })
    }

    /// Flushes all pending writes to disk.
    pub fn flush(&self) -> Result<(), Error> {
        self.db.flush()?;
        self.key_shares.flush()?;
        Ok(())
    }

    /// Gets all key shares for a specific multisig.
    ///
    /// # Arguments
    ///
    /// * `multisig_id` - The identifier for the multisig.
    ///
    /// # Returns
    ///
    /// Returns `Some(MultisigKeyShares)` if found, `None` if not found.
    pub fn get_key_shares(&self, _multisig_id: &[u8]) -> Result<Option<MultisigKeyShares>, Error> {
        // TODO: Implement deserialization
        // if let Some(b) = self.key_shares.get(_multisig_id)? {
        //     let shares = ciborium::from_reader::<MultisigKeyShares, _>(b.as_ref())?;
        //     Ok(Some(shares))
        // } else {
        //     Ok(None)
        // }
        todo!("Implement get_key_shares deserialization")
    }

    /// Stores key shares for a specific multisig.
    /// This will overwrite any existing shares for this multisig.
    ///
    /// # Arguments
    ///
    /// * `multisig_id` - The identifier for the multisig.
    /// * `shares` - The MultisigKeyShares to store.
    pub fn set_key_shares(
        &self,
        _multisig_id: &[u8],
        _shares: MultisigKeyShares,
    ) -> Result<(), Error> {
        // TODO: Implement serialization
        // let mut bytes = Vec::new();
        // ciborium::into_writer(&_shares, &mut bytes).expect("writing to buffer");
        // self.key_shares.insert(_multisig_id, &bytes[..])?;
        // Ok(())
        todo!("Implement set_key_shares serialization")
    }

    /// Adds or updates a single key share within a multisig's shares.
    ///
    /// # Arguments
    ///
    /// * `multisig_id` - The identifier for the multisig.
    /// * `node_id` - The identifier for the node.
    /// * `key_package` - The FROST key package to store.
    pub fn add_key_share(
        &self,
        _multisig_id: &[u8],
        _node_id: &[u8],
        _key_package: frost::keys::KeyPackage,
    ) -> Result<(), Error> {
        // TODO: Implement:
        // 1. Get existing MultisigKeyShares (or create new)
        // 2. Insert/update the _key_package in the BTreeMap
        // 3. Serialize and store back
        todo!("Implement add_key_share")
    }

    /// Gets a specific key share for a multisig and node.
    ///
    /// # Arguments
    ///
    /// * `multisig_id` - The identifier for the multisig.
    /// * `node_id` - The identifier for the node.
    ///
    /// # Returns
    ///
    /// Returns the key package if found.
    pub fn get_key_share(
        &self,
        _multisig_id: &[u8],
        _node_id: &[u8],
    ) -> Result<Option<frost::keys::KeyPackage>, Error> {
        // TODO: Implement:
        // 1. Get MultisigKeyShares for _multisig_id
        // 2. Lookup _node_id in the BTreeMap
        // 3. Return cloned key package
        todo!("Implement get_key_share")
    }

    /// Removes a specific key share.
    ///
    /// # Arguments
    ///
    /// * `multisig_id` - The identifier for the multisig.
    /// * `node_id` - The identifier for the node.
    pub fn remove_key_share(&self, _multisig_id: &[u8], _node_id: &[u8]) -> Result<(), Error> {
        // TODO: Implement:
        // 1. Get existing MultisigKeyShares
        // 2. Remove _node_id from BTreeMap
        // 3. If empty, remove entire multisig, otherwise store back
        todo!("Implement remove_key_share")
    }

    /// Removes all key shares for a multisig.
    ///
    /// # Arguments
    ///
    /// * `multisig_id` - The identifier for the multisig.
    pub fn remove_multisig(&self, _multisig_id: &[u8]) -> Result<(), Error> {
        // TODO: Implement
        // self.key_shares.remove(_multisig_id)?;
        // Ok(())
        todo!("Implement remove_multisig")
    }

    /// Lists all multisig IDs that have key shares stored.
    pub fn list_multisig_ids(&self) -> Result<Vec<Vec<u8>>, Error> {
        // TODO: Implement by iterating over key_shares tree keys
        todo!("Implement list_multisig_ids")
    }

    /// Exports all key shares in an encrypted format.
    ///
    /// # Arguments
    ///
    /// * `passphrase` - The passphrase to encrypt the export.
    ///
    /// # Returns
    ///
    /// Returns `ExportedKeyShares` if there are any shares to export,
    /// `None` if the database is empty.
    pub fn export_key_shares(
        &self,
        _passphrase: Zeroizing<String>,
    ) -> Result<Option<ExportedKeyShares>, Error> {
        // TODO: Implement encryption following btc-server's pattern:
        // 1. Collect all MultisigKeyShares into a BTreeMap<multisig_id, MultisigKeyShares>
        // 2. Serialize to bytes
        // 3. Generate random nonce
        // 4. Derive encryption key from _passphrase using Merlin transcript
        // 5. Encrypt with ChaCha20Poly1305
        // 6. Return ExportedKeyShares
        todo!("Implement export_key_shares encryption")
    }

    /// Imports key shares from an encrypted export.
    ///
    /// # Arguments
    ///
    /// * `passphrase` - The passphrase to decrypt the import.
    /// * `export` - The encrypted key shares export.
    pub fn import_key_shares(
        &self,
        _passphrase: Zeroizing<String>,
        _export: ExportedKeyShares,
    ) -> Result<(), Error> {
        // TODO: Implement decryption following btc-server's pattern:
        // 1. Check version matches EXPORTED_PACKAGE_VERSION
        // 2. Derive decryption key from _passphrase using Merlin transcript
        // 3. Decrypt with ChaCha20Poly1305
        // 4. Deserialize to BTreeMap<multisig_id, MultisigKeyShares>
        // 5. Store each MultisigKeyShares
        todo!("Implement import_key_shares decryption")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_creation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db = Db::open(temp_dir.path()).unwrap();
        assert!(db.flush().is_ok());
    }

    #[test]
    fn test_multisig_key_shares_serialization() {
        // TODO: Test that MultisigKeyShares can be serialized/deserialized
    }

    #[test]
    fn test_export_import_roundtrip() {
        // TODO: Test that export -> import preserves data
    }
}
