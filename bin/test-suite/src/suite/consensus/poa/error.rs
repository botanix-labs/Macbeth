use displaydoc::Display as DisplayDoc;
use thiserror::Error;

/// Error enum representing all errors returned by the library
#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Tokio handle join Error
    TokioJoinError(tokio::task::JoinError),
}
