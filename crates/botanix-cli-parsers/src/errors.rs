/// Error thrown while parsing a URL
#[derive(thiserror::Error, Debug)]
pub enum UrlParsingError {
    /// Failed to parse the address
    #[error("Could not parse URL from {0}")]
    Parse(String),
}

/// Error that can occur when parsing an address.
#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum EthereumAddressParseError {
    /// Does not have exactly 40 hex characters.
    #[error("Invalid hex length {0}")]
    InvalidHexLength(usize),
    /// Decoded byte array does not contain exactly 20 bytes.
    #[error("Incorrect byte count {0}")]
    IncorrectByteCount(usize),
    /// Input string contains invalid hex characters.
    #[error("Invalid hex string")]
    InvalidHex,
    /// Checksum validation failed.
    #[error("Checksum validation failed")]
    ChecksumFailed,
}
