use displaydoc::Display as DisplayDoc;
use thiserror::Error;
use tonic::Status;

/// Error enum representing all errors returned by the library
#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Grpc Server Connect Error {0}
    ServerConnect(tonic::transport::Error),
    /// Grpc Request Error {0}
    Request(Status),
    /// Invalid Btc Server port
    InvalidBtcServerPort,
    /// Public Key Parse Error {0}
    PubKeyParse(bitcoin::secp256k1::Error),
    /// Public Key Mismatch
    PublicKeyMismatch,
}
