use displaydoc::Display as DisplayDoc;
use thiserror::Error;
use tonic::Status;

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
    /// Round 1 Packages Expected Length Mismatch
    Round1PackagesLenghtMismatch,
    /// Round 2 Packages Expected Length Mismatch
    Round2PackagesLenghtMismatch,
    /// Pegin Notification Error
    PeginNotification,
}
