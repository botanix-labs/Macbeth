use std::{collections::BTreeMap, path::Path};

#[allow(unused_imports)]
use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit, Nonce};
use frost_secp256k1_tr as frost;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

// Import from btc-server for compatibility
use btcserverlib::database::{ExportedKeyPackage, EXPORTED_PACKAGE_VERSION};

pub mod error;
pub use error::Error;

/// Tree identifier for key shares storage.
const TREE_KEY_SHARES: &[u8; 9] = b"keyshares";

/// Represents a collection of key shares for a single multisig.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MultisigKeyShares {
    /// Map of node_id -> FROST key package
    pub shares: BTreeMap<Vec<u8>, frost::keys::KeyPackage>,
}

/// Database handle for the pegin-recovery service.
#[derive(Clone, Debug)]
pub struct Db {
    /// The underlying sled database.
    db: sled::Db,
    /// Tree for storing key shares, indexed by multisig_id.
    key_shares: sled::Tree,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Db, sled::Error> {
        let db = sled::open(path)?;
        Ok(Db { key_shares: db.open_tree(TREE_KEY_SHARES)?, db })
    }

    pub fn flush(&self) -> Result<(), Error> {
        self.db.flush()?;
        self.key_shares.flush()?;
        Ok(())
    }

    pub fn get_key_shares(&self, multisig_id: &[u8]) -> Result<Option<MultisigKeyShares>, Error> {
        if let Some(b) = self.key_shares.get(multisig_id)? {
            let shares = ciborium::from_reader::<MultisigKeyShares, _>(b.as_ref())?;
            Ok(Some(shares))
        } else {
            Ok(None)
        }
    }

    /// Overwrites any existing shares for this multisig.
    pub fn set_key_shares(
        &self,
        multisig_id: &[u8],
        shares: MultisigKeyShares,
    ) -> Result<(), Error> {
        let mut bytes = Vec::new();
        ciborium::into_writer(&shares, &mut bytes).expect("writing to buffer");
        self.key_shares.insert(multisig_id, &bytes[..])?;
        Ok(())
    }

    pub fn add_key_share(
        &self,
        multisig_id: &[u8],
        node_id: &[u8],
        key_package: frost::keys::KeyPackage,
    ) -> Result<(), Error> {
        let mut multisig_shares = self
            .get_key_shares(multisig_id)?
            .unwrap_or_else(|| MultisigKeyShares { shares: BTreeMap::new() });
        multisig_shares.shares.insert(node_id.to_vec(), key_package);
        self.set_key_shares(multisig_id, multisig_shares)?;
        Ok(())
    }

    pub fn get_key_share(
        &self,
        multisig_id: &[u8],
        node_id: &[u8],
    ) -> Result<Option<frost::keys::KeyPackage>, Error> {
        if let Some(multisig_shares) = self.get_key_shares(multisig_id)? {
            Ok(multisig_shares.shares.get(node_id).cloned())
        } else {
            Ok(None)
        }
    }

    /// Imports a key package from btc-server's ExportedKeyPackage format.
    /// Uses the same crypto (Merlin + ChaCha20Poly1305) as btc-server for compatibility.
    pub fn import_from_btc_server(
        &self,
        multisig_id: &[u8],
        node_identifier: frost::Identifier,
        passphrase: Zeroizing<String>,
        export: ExportedKeyPackage,
    ) -> Result<(), Error> {
        if export.version != EXPORTED_PACKAGE_VERSION {
            return Err(Error::BadExportedPackageFormatVersion);
        }

        let nonce = Nonce::from_slice(&export.iv);

        // Derive decryption key using Merlin transcript (matches btc-server)
        let mut t = merlin::Transcript::new(b"Botanix_Macbeth_BtcServer_ExportedKeyPackage");
        t.append_message(b"salt", nonce);
        t.append_message(b"passphrase", passphrase.as_bytes());

        let mut master = Zeroizing::new([0u8; 32]);
        t.challenge_bytes(b"secret_key_package", master.as_mut_slice());

        // Decrypt and deserialize
        let cipher = ChaCha20Poly1305::new_from_slice(master.as_slice())
            .expect("master key must be 32-bytes");

        let decrypted = cipher
            .decrypt(nonce, export.enc_key_package.as_slice())
            .map_err(|_| Error::BadDecryptionPassphrase)?;

        let key_package: frost::keys::KeyPackage = ciborium::from_reader(decrypted.as_slice())?;

        // Store it
        let node_id = node_identifier.serialize();
        self.add_key_share(multisig_id, &node_id, key_package)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to create a dummy FROST key package for testing
    fn create_test_key_package(seed: u8) -> frost::keys::KeyPackage {
        // Generate a simple key package using FROST's test helpers
        // We use a deterministic seed to ensure reproducible tests
        use frost::rand_core::OsRng;
        let mut rng = OsRng;
        let max_signers: u16 = 3;
        let min_signers: u16 = 2;

        // Use trusted dealer for testing
        let (shares, _pubkeys) = frost::keys::generate_with_dealer(
            max_signers,
            min_signers,
            frost::keys::IdentifierList::Default,
            &mut rng,
        )
        .unwrap();

        // Return the first share, offset by seed for variety
        let identifier =
            frost::Identifier::try_from((((seed as u16) % max_signers) + 1) as u16).unwrap();
        frost::keys::KeyPackage::try_from(shares.get(&identifier).unwrap().clone()).unwrap()
    }

    #[test]
    fn test_db_creation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db = Db::open(temp_dir.path()).unwrap();
        assert!(db.flush().is_ok());
    }

    #[test]
    fn test_add_and_get_key_share() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db = Db::open(temp_dir.path()).unwrap();

        let multisig_id = b"test_multisig_1";
        let node_id = frost::Identifier::try_from(1u16).unwrap().serialize();
        let key_package = create_test_key_package(1);

        // Add a key share
        db.add_key_share(multisig_id, &node_id, key_package.clone()).unwrap();

        // Retrieve it
        let retrieved = db.get_key_share(multisig_id, &node_id).unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap(), key_package);
    }

    #[test]
    fn test_add_multiple_key_shares() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db = Db::open(temp_dir.path()).unwrap();

        let multisig_id = b"test_multisig_2";
        let node_id_1 = frost::Identifier::try_from(1u16).unwrap().serialize();
        let node_id_2 = frost::Identifier::try_from(2u16).unwrap().serialize();
        let node_id_3 = frost::Identifier::try_from(3u16).unwrap().serialize();

        let key_package_1 = create_test_key_package(1);
        let key_package_2 = create_test_key_package(2);
        let key_package_3 = create_test_key_package(3);

        // Add multiple key shares for the same multisig
        db.add_key_share(multisig_id, &node_id_1, key_package_1.clone()).unwrap();
        db.add_key_share(multisig_id, &node_id_2, key_package_2.clone()).unwrap();
        db.add_key_share(multisig_id, &node_id_3, key_package_3.clone()).unwrap();

        // Verify all are stored
        let retrieved_1 = db.get_key_share(multisig_id, &node_id_1).unwrap().unwrap();
        let retrieved_2 = db.get_key_share(multisig_id, &node_id_2).unwrap().unwrap();
        let retrieved_3 = db.get_key_share(multisig_id, &node_id_3).unwrap().unwrap();

        assert_eq!(retrieved_1, key_package_1);
        assert_eq!(retrieved_2, key_package_2);
        assert_eq!(retrieved_3, key_package_3);

        // Verify get_key_shares returns all
        let all_shares = db.get_key_shares(multisig_id).unwrap().unwrap();
        assert_eq!(all_shares.shares.len(), 3);
    }

    #[test]
    fn test_update_existing_key_share() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db = Db::open(temp_dir.path()).unwrap();

        let multisig_id = b"test_multisig_3";
        let node_id = frost::Identifier::try_from(1u16).unwrap().serialize();
        let key_package_1 = create_test_key_package(1);
        let key_package_2 = create_test_key_package(2);

        // Add initial key share
        db.add_key_share(multisig_id, &node_id, key_package_1).unwrap();

        // Update it with a new key package
        db.add_key_share(multisig_id, &node_id, key_package_2.clone()).unwrap();

        // Verify the updated value
        let retrieved = db.get_key_share(multisig_id, &node_id).unwrap().unwrap();
        assert_eq!(retrieved, key_package_2);

        // Verify still only one share
        let all_shares = db.get_key_shares(multisig_id).unwrap().unwrap();
        assert_eq!(all_shares.shares.len(), 1);
    }

    #[test]
    fn test_get_nonexistent_key_share() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db = Db::open(temp_dir.path()).unwrap();

        let multisig_id = b"nonexistent_multisig";
        let node_id = frost::Identifier::try_from(1u16).unwrap().serialize();

        // Try to get a key share that doesn't exist
        let result = db.get_key_share(multisig_id, &node_id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_multisig_key_shares_serialization() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db = Db::open(temp_dir.path()).unwrap();

        let multisig_id = b"test_multisig_7";
        let node_id = frost::Identifier::try_from(1u16).unwrap().serialize();
        let key_package = create_test_key_package(1);

        // Create MultisigKeyShares directly
        let mut shares = BTreeMap::new();
        shares.insert(node_id.to_vec(), key_package.clone());
        let multisig_shares = MultisigKeyShares { shares };

        // Store and retrieve
        db.set_key_shares(multisig_id, multisig_shares.clone()).unwrap();
        let retrieved = db.get_key_shares(multisig_id).unwrap().unwrap();

        // Verify serialization roundtrip
        assert_eq!(retrieved, multisig_shares);
    }

    #[test]
    fn test_import_from_btc_server() {
        use btcserverlib::database::Db as BtcDb;

        let temp_dir_btc = tempfile::tempdir().unwrap();
        let temp_dir_recovery = tempfile::tempdir().unwrap();

        // Setup: Create and export a key from btc-server
        let passphrase = Zeroizing::new("test_passphrase_123".to_string());
        let key_package = create_test_key_package(1);

        let export = {
            let btc_db = BtcDb::open(temp_dir_btc.path()).unwrap();

            // Store key package in btc-server
            btc_db.set_key_package(key_package.clone()).unwrap();

            // Create a dummy public key package (required for export)
            use frost::rand_core::OsRng;
            let mut rng = OsRng;
            let (_, pk_package) = frost::keys::generate_with_dealer(
                3,
                2,
                frost::keys::IdentifierList::Default,
                &mut rng,
            )
            .unwrap();
            btc_db.set_pubkey_package(pk_package).unwrap();

            // Export
            btc_db.export_key_package(passphrase.clone()).unwrap().unwrap()
        };

        // Import into pegin-recovery
        let recovery_db = Db::open(temp_dir_recovery.path()).unwrap();
        let multisig_id = b"imported_multisig";
        let node_identifier = frost::Identifier::try_from(1u16).unwrap();

        recovery_db
            .import_from_btc_server(multisig_id, node_identifier, passphrase, export)
            .unwrap();

        // Verify the import worked
        let node_id = node_identifier.serialize();
        let retrieved = recovery_db.get_key_share(multisig_id, &node_id).unwrap().unwrap();
        assert_eq!(retrieved, key_package);
    }
}
