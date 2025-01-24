use bitcoin::psbt::ExtractTxError;
use thiserror::Error;

use frost_secp256k1_tr as frost;

use crate::{database, wallet::psbt::CalculateSighashError};

#[derive(Debug, Error)]
pub enum SigningError {
    #[error("round 1 error: {0}")]
    Round1(#[from] SigningRound1Error),
    #[error("round 2 error: {0}")]
    Round2(#[from] SigningRound2Error),
}

#[derive(Debug, Error)]
pub enum SigningRound1Error {
    #[error("already in signing session")]
    AlreadyInSigningSession,
    #[error("missing key package")]
    MissingKeyPackage,
    #[error("invalid number of signing nonces requested")]
    InvalidNumberOfNoncesRequested,
    #[error("invalid signing package: {0}")]
    InvalidSigningPackage(&'static str),
    #[error("internal DB error")]
    Db(#[from] database::Error),
    #[error("failed to add signing commits to psbt")]
    FailedToAddSigningCommitsToPsbt(
        #[from] crate::wallet::psbt::PsbtToSigningPackageConversionError,
    ),
    #[error("failed to get smart estimate fee rate")]
    FailedToGetEstimateSmartFeeRate,
    #[error("fee rate difference is too great")]
    FeeRateDifferenceTooGreat,
    #[error("failed to validate psbt: {0}")]
    FailedToValidatePsbt(#[from] crate::util::ValidatePSBTError),
    #[error("extract tx error: {0}")]
    ExtractTxError(#[from] ExtractTxError),
    #[error("failed to get fee rate from psbt")]
    FailedToGetFeeRateFromPsbt,
    #[error("missing signing package at index: {0}")]
    MissingSigningPackageAtIndex(usize),
}

#[derive(Debug, Error)]
pub enum SigningRound2Error {
    #[error("missing key package")]
    MissingKeyPackage,
    #[error("invalid signing package: {0}")]
    InvalidSigningPackage(&'static str),
    #[error("internal FROST error: {0}")]
    FrostError(#[from] frost::Error),
    #[error("missing round 1 signing nonces")]
    MissingRound1SigningNonce,
    #[error("internal DB error")]
    Db(#[from] database::Error),
    #[error("signer not found in signing package at index: {0}")]
    SignerNotFound(usize),
    #[error("Failed to calculate sighash: {0}")]
    FailedToCalculateSighash(#[from] CalculateSighashError),
    #[error("Failed parse out to sign package: {0}")]
    PsbtToSigningPackageConversionError(
        #[from] crate::wallet::psbt::PsbtToSigningPackageConversionError,
    ),
    #[error("failed to validate psbt: {0}")]
    FailedToValidatePsbt(#[from] crate::util::ValidatePSBTError),
    #[error("extract tx error: {0}")]
    ExtractTxError(#[from] ExtractTxError),
}


// Currently not used
// #[derive(Debug, Error)]
// pub enum SigningFinalizeError {
//     #[error("missing key package")]
//     MissingKeyPackage,
//     #[error("too many witness items")]
//     TooManyWitnessItems,
//     #[error("PSBT finalization failed : {0:?}")]
//     PsbtFinalizationFailed(Vec<PsbtError>),
//     #[error("Taproot Signature validation error: {0}")]
//     TaprootSignatureValidationError(#[from] bitcoin::taproot::TaprootError),
//     #[error("internal DB error")]
//     DbError(#[from] database::Error),
//     #[error("sig from slice error: {0}")]
//     SigFromSliceError(#[from] SigFromSliceError),
//     #[error("extract tx error: {0}")]
//     ExtractTxError(#[from] ExtractTxError),
//     #[error("coordinator internal error: {0}")]
//     CoordinatorError(#[from] CoordinatorError),
//     #[error("missing pending pegout: {0}")]
//     MissingPendingPegout(PegoutId),
//     #[error("psbt pegout missing pegout id")]
//     MissingPsbtPegout,
//     #[error("psbt includes invalid change output")]
//     InvalidChangeOutput,
//     #[error("expecting only one change output")]
//     ExpectingOnlyOneChangeOutput,
//     #[error("psbt to signing package conversion error: {0}")]
//     PsbtToSigningPackageConversionError(
//         #[from] crate::wallet::psbt::PsbtToSigningPackageConversionError,
//     ),
//     #[error("FROST error: {0}")]
//     FrostError(#[from] frost::Error),
//     #[error("failed to validate outputs: {0}")]
//     ValidateOutputsError(#[from] ValidateOutputsError),
// }

