use displaydoc::Display as DisplayDoc;
use serde_json::error::Error as SerdeError;
use thiserror::Error;

/// Password hashing error types.
#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Serde error: `{0}`
    Serde(#[from] SerdeError),
}
