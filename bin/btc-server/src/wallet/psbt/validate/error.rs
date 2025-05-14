use crate::pegout_id::PegoutId;
use crate::wallet::psbt::{PsbtFeePerOutputError, PsbtOutputExtError};
use bitcoin::psbt::Output;

// TODO: Add data to error message?
#[derive(thiserror::Error, Debug)]
pub enum PsbtValidationError {
    #[error("Outputs aren't present")]
    NoOutputs,
    #[error("Pegout outputs aren't present")]
    NoPegoutOutputs,
    #[error("Change output isn't present")]
    NoChangeOutput,
    #[error("PSBT output count {psbt_outputs_count} aren't matching to unsigned transaction output count {unsigned_tx_outputs_count}")]
    OutputCountMismatch { psbt_outputs_count: usize, unsigned_tx_outputs_count: usize },
    #[error("PSBT has more than one change output")]
    ExpectingOnlyOneChangeOutput { first_output: Output, second_output: Output },
    #[error("Pegout ID bytes are invalid")]
    InvalidPegoutIdBytes { error: PsbtOutputExtError, output: Output },
    #[error("Duplicate pegout ID output")]
    DuplicatePegoutId(PegoutId),
    // TODO: Isn't a code error rather than a validation error?
    #[error("Failed to calculate fee per output: {0}")]
    FeePerOutputCalculationFailed(PsbtFeePerOutputError),
    #[error("Pegout {0} already finalized")]
    AlreadyFinalizedPegout(PegoutId),
    #[error("Pegout ID bytes are invalid")]
    PendingPegoutDataNotFound(PegoutId),
    #[error(
        "Corresponding unsigned transaction output index {index} not found for pegout {pegout_id}"
    )]
    UnsignedTxOutputNotFound { index: usize, pegout_id: PegoutId },
    #[error("Pegout output script pubkey does not match destination public key")]
    PegoutOutputScriptPubkeyMismatch {
        pegout_id: PegoutId,
        expected: bitcoin::ScriptBuf,
        actual: bitcoin::ScriptBuf,
    },
    Pegout


Calculating expected amount caused an underflow
"The output value does not match the expected amount
}
