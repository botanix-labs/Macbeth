use crate::{
    database::Error as DbError,
    util::ValidatePSBTError,
    wallet::{psbt::CalculateSighashError, util::VerifyingKeyExtError},
};
use bitcoin::{hashes::sha256, psbt::ExtractTxError};
use frost_secp256k1_tr::{self as frost};
use miniscript::psbt::Error as PsbtError;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoordinatorError {
    #[error("missing key package")]
    MissingKeyPackage,
    #[error("invalid frost peer id")]
    InvalidFrostPeerId,
    #[error("not enough signers")]
    NotEnoughSigners,
    #[error("invalid signing package: {0}")]
    InvalidSigningPackage(&'static str),
    #[error("failed to convert verifying key to secp pk")]
    FailedToConvertVerifyingKeyToSecpPk(#[from] VerifyingKeyExtError),
    #[error("Failed to calculate sighash: {0}")]
    FailedToCalculateSighash(#[from] CalculateSighashError),
    #[error("Pbst error: {0}")]
    Pbst(#[from] PsbtError),
    #[error("internal FROST error: {0}")]
    FrostError(#[from] frost::Error),
    #[error("internal DB error")]
    Db(#[from] DbError),
    #[error("PSBT finalization failed : {0:?}")]
    PbstFinalizationFailed(Vec<PsbtError>),
    #[error("Invalid resulting transaction")]
    InvaildResultingTx,
    #[error("Failed parse out to sign package: {0}")]
    PsbtToSigningPackageConversionError(
        #[from] crate::wallet::psbt::PsbtToSigningPackageConversionError,
    ),
    #[error("Could not find psbt")]
    CouldNotFindPsbt,
    #[error("Failed to broadcast tx: {0}")]
    FailedToBroadcastTx(bitcoincore_rpc::Error),
    #[error("Could not find participant information")]
    CouldNotFindParticipantInformation(),
    #[error("Failed to validate psbt: {0}")]
    FailedToValidatePsbt(#[from] ValidatePSBTError),
    #[error("extract tx error: {0}")]
    ExtractTxError(#[from] ExtractTxError),
    #[error("pegout mgr sync: {0}")]
    PegoutMgrSync(#[from] crate::pegout_scheduler::SyncError),
    #[error("utxo merkle root mismatch: expected {expected}, actual {actual:?}")]
    UtxoMerkleRootMismatch { expected: sha256::Hash, actual: sha256::Hash },
    #[error("Secp256k1 error: {0}")]
    Secp256k1Error(#[from] bitcoin::secp256k1::Error),
    #[error("Missing final script")]
    MissingFinalScript,
    #[error("missing signing package at index: {0}")]
    MissingSigningPackageAtIndex(usize),
    #[error("coin selection error: {0}")]
    CoinSelectionErr(#[from] crate::wallet::coin_selection::CoinSelectionError),
    #[error("No conflicting inputs exist")]
    NoConflictingInputs,
    #[error("Missing utxo for conflicting input")]
    MissingUtxoForConflictingInput,
    #[error("All outputs are dust")]
    AllOutputsAreDust,
}
