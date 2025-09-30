use bitcoin::psbt::{self};
use frost_secp256k1_tr as frost;
use std::{array::TryFromSliceError, io, time::SystemTimeError};
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
    Serialization(#[from] TryFromSliceError),
    #[error("bitcoin serialization error {0}")]
    BitcoinSerialization(#[from] bitcoin::consensus::encode::Error),
    #[error("PSBT error: {0}")]
    Psbt(#[from] psbt::Error),
    #[error("Transaction error: {0}")]
    Transaction(String),
    #[error("Rpc to db data mapping error: {0}")]
    RpcToDbMap(String),
    #[error("empty merkle root")]
    EmptyMerkleRoot,
    #[error("Ciborium write error {0}")]
    CiboriumWrite(#[from] ciborium::ser::Error<std::io::Error>),
    #[error("expected output at index but not found")]
    OutputNotFound(usize),
    #[error("hash engine error {0}")]
    HashEngine(#[from] std::io::Error),
    #[error("Invalid UTXO version number {0}")]
    InvalidUTXOVersion(u32),
    #[error("Error getting duration since epoch: {0}")]
    DurationSinceEpoch(#[from] SystemTimeError),
    #[error("Failed to get tx out for input: {0}")]
    BitcoindError(#[from] bitcoincore_rpc::Error),
    #[error("Tracked tx not found in Pegout Scheduler")]
    TrackedTxNotFoundInPegoutScheduler,
    #[error("Bad passphrase for decrypting key-package import")]
    BadDecryptionPassphrase,
    /// Related to [`super::ExportedKeyPackage`].
    #[error("Bad exported package format version")]
    BadExportedPackageFormatVersion,
}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

impl From<sled::transaction::TransactionError<sled::Error>> for Error {
    fn from(e: sled::transaction::TransactionError<sled::Error>) -> Error {
        match e {
            sled::transaction::TransactionError::Abort(e) => Error::Db(e),
            sled::transaction::TransactionError::Storage(e) => Error::Db(e),
        }
    }
}
