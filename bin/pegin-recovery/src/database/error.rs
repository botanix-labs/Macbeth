use frost_secp256k1_tr as frost;
use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("internal DB error")]
    Db(#[from] sled::Error),

    #[error("data corruption error")]
    DataCorruption(#[from] ciborium::de::Error<io::Error>),

    #[error("Frost serialization error {0}")]
    FrostSerialization(#[from] frost::Error),

    #[error("Serialization error {0}")]
    Serialization(String),

    #[error("Ciborium write error {0}")]
    CiboriumWrite(#[from] ciborium::ser::Error<std::io::Error>),

    #[error("Bad passphrase for decrypting key-package import")]
    BadDecryptionPassphrase,

    #[error("Bad exported package format version")]
    BadExportedPackageFormatVersion,

    #[error("Key share not found for multisig_id={0}, node_id={1}")]
    KeyShareNotFound(String, String),

    #[error("No key shares found for multisig_id={0}")]
    NoKeySharesForMultisig(String),
}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}
