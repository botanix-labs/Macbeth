use displaydoc::Display as DisplayDoc;
use thiserror::Error;

#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Tokio handle join Error
    TokioJoinError(tokio::task::JoinError),
}
