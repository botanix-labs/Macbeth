use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{hex, Address, B256};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs},
    str::FromStr,
    time::Duration,
};
use url::Url;

/// Helper to parse a [Duration] from seconds
pub fn parse_duration_from_secs(arg: &str) -> eyre::Result<Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(Duration::from_secs(seconds))
}

/// Helper to parse a [Duration] from seconds if it's a number or milliseconds if the input contains
/// a `ms` suffix:
///  * `5ms` -> 5 milliseconds
///  * `5` -> 5 seconds
///  * `5s` -> 5 seconds
pub fn parse_duration_from_secs_or_ms(
    arg: &str,
) -> eyre::Result<Duration, std::num::ParseIntError> {
    if arg.ends_with("ms") {
        arg.trim_end_matches("ms").parse::<u64>().map(Duration::from_millis)
    } else if arg.ends_with('s') {
        arg.trim_end_matches('s').parse::<u64>().map(Duration::from_secs)
    } else {
        arg.parse::<u64>().map(Duration::from_secs)
    }
}

/// Parse [`BlockHashOrNumber`]
pub fn hash_or_num_value_parser(value: &str) -> eyre::Result<BlockHashOrNumber, eyre::Error> {
    match B256::from_str(value) {
        Ok(hash) => Ok(BlockHashOrNumber::Hash(hash)),
        Err(_) => Ok(BlockHashOrNumber::Number(value.parse()?)),
    }
}

/// Parse a [`SocketAddr`] from a `str` prefixing with http.
///
/// An error is returned if the value is empty or if non socket value is passed
pub fn parse_grpc_address(value: &str) -> eyre::Result<String, SocketAddressParsingError> {
    if value.is_empty() {
        return Err(SocketAddressParsingError::Empty);
    }
    // TODO configurable for https
    let addr = format!("http://{}", value);
    tonic::transport::Endpoint::try_from(addr.clone()).map_err(|_e| {
        SocketAddressParsingError::Parse("Failed to parse as tonic endpoint".to_string())
    })?;
    Ok(addr)
}

/// Error thrown while parsing a socket address.
#[derive(thiserror::Error, Debug)]
pub enum SocketAddressParsingError {
    /// Failed to convert the string into a socket addr
    #[error("could not parse socket address: {0}")]
    Io(#[from] std::io::Error),
    /// Input must not be empty
    #[error("cannot parse socket address from empty string")]
    Empty,
    /// Failed to parse the address
    #[error("could not parse socket address from {0}")]
    Parse(String),
    /// Failed to parse port
    #[error("could not parse port: {0}")]
    Port(#[from] std::num::ParseIntError),
}

/// Parse a [URL] from a `str` value
pub fn parse_url(value: &str) -> eyre::Result<Url, UrlParsingError> {
    let url = Url::parse(value).map_err(|_e| UrlParsingError::Parse(value.to_owned()))?;
    Ok(url)
}

/// Error thrown while parsing a URL
#[derive(thiserror::Error, Debug)]
pub enum UrlParsingError {
    /// Failed to parse the address
    #[error("Could not parse URL from {0}")]
    Parse(String),
}

/// Parse a [`SocketAddr`] from a `str`.
///
/// The following formats are checked:
///
/// - If the value can be parsed as a `u16` or starts with `:` it is considered a port, and the
///   hostname is set to `localhost`.
/// - If the value contains `:` it is assumed to be the format `<host>:<port>`
/// - Otherwise it is assumed to be a hostname
///
/// An error is returned if the value is empty.
pub fn parse_socket_address(value: &str) -> eyre::Result<SocketAddr, SocketAddressParsingError> {
    if value.is_empty() {
        return Err(SocketAddressParsingError::Empty);
    }

    if let Some(port) = value.strip_prefix(':').or_else(|| value.strip_prefix("localhost:")) {
        let port: u16 = port.parse()?;
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
    }
    if let Ok(port) = value.parse::<u16>() {
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
    }
    value
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| SocketAddressParsingError::Parse(value.to_string()))
}

/// Attempts to parse a hex string into an Address.
/// Accepts an optional "0x" prefix.
pub fn parse_ethereum_address(s: &str) -> eyre::Result<Address, EthereumAddressParseError> {
    // Remove the optional "0x" prefix.
    let hex_str = s.strip_prefix("0x").unwrap_or(s);

    // Validate length: addresses must have 40 hex characters.
    if hex_str.len() != 40 {
        return Err(EthereumAddressParseError::InvalidHexLength(hex_str.len()));
    }

    // Decode the hex string into bytes.
    let bytes = hex::decode(hex_str).map_err(|_e| EthereumAddressParseError::InvalidHex)?;

    // Ensure the decoded bytes are exactly 20 in length.
    if bytes.len() != 20 {
        return Err(EthereumAddressParseError::IncorrectByteCount(bytes.len()));
    }

    // Checksum the address.
    if Address::parse_checksummed(format!("0x{}", hex_str), None).is_err() {
        return Err(EthereumAddressParseError::ChecksumFailed);
    }

    // Create an Address.
    let mut addr_array = [0u8; 20];
    addr_array.copy_from_slice(&bytes);
    Ok(Address::from(addr_array))
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

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[test]
    fn parse_socket_addresses() {
        for value in ["localhost:9000", ":9000", "9000"] {
            let socket_addr = parse_socket_address(value)
                .unwrap_or_else(|_| panic!("could not parse socket address: {value}"));

            assert!(socket_addr.ip().is_loopback());
            assert_eq!(socket_addr.port(), 9000);
        }
    }

    #[test]
    fn parse_socket_address_random() {
        let port: u16 = rand::thread_rng().gen();

        for value in [format!("localhost:{port}"), format!(":{port}"), port.to_string()] {
            let socket_addr = parse_socket_address(&value)
                .unwrap_or_else(|_| panic!("could not parse socket address: {value}"));

            assert!(socket_addr.ip().is_loopback());
            assert_eq!(socket_addr.port(), port);
        }
    }

    #[test]
    fn parse_ms_or_seconds() {
        let ms = parse_duration_from_secs_or_ms("5ms").unwrap();
        assert_eq!(ms, Duration::from_millis(5));

        let seconds = parse_duration_from_secs_or_ms("5").unwrap();
        assert_eq!(seconds, Duration::from_secs(5));

        let seconds = parse_duration_from_secs_or_ms("5s").unwrap();
        assert_eq!(seconds, Duration::from_secs(5));

        assert!(parse_duration_from_secs_or_ms("5ns").is_err());
    }

    #[test]
    fn test_parse_ethereum_address_no_prefix() {
        // A valid address with 40 hex characters (without the "0x" prefix)
        let address_str = "43C8bDCb9AFeBB1D834A7de18CC214a6FD1632d9";
        let parsed = parse_ethereum_address(address_str).expect("Should parse valid address");

        // Build the expected Address by decoding and converting.
        let expected_bytes = hex::decode(address_str).unwrap();
        let mut expected_array = [0u8; 20];
        expected_array.copy_from_slice(&expected_bytes);
        let expected_address = Address::from(expected_array);

        assert_eq!(parsed, expected_address);
    }

    #[test]
    fn test_parse_ethereum_address_with_prefix() {
        // A valid address provided with the "0x" prefix.
        let address_str = "0x43C8bDCb9AFeBB1D834A7de18CC214a6FD1632d9";
        let parsed = parse_ethereum_address(address_str).expect("Should parse valid address");

        // The expected address is the same as if the prefix weren't there.
        let trimmed = "43C8bDCb9AFeBB1D834A7de18CC214a6FD1632d9";
        let expected_bytes = hex::decode(trimmed).unwrap();
        let mut expected_array = [0u8; 20];
        expected_array.copy_from_slice(&expected_bytes);
        let expected_address = Address::from(expected_array);

        assert_eq!(parsed, expected_address);
    }

    #[test]
    fn test_parse_ethereum_address_invalid_length() {
        // Test with too short a string.
        let address_str = "1234";
        let err = parse_ethereum_address(address_str).unwrap_err();
        assert_eq!(err, EthereumAddressParseError::InvalidHexLength(address_str.len()));
    }

    #[test]
    fn test_parse_ethereum_address_invalid_hex_characters() {
        // Test with invalid hex characters ("Z" is invalid in hexadecimal).
        let address_str = "0xZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ";
        let err = parse_ethereum_address(address_str).unwrap_err();
        match err {
            EthereumAddressParseError::InvalidHex => {}
            _ => panic!("Expected InvalidHex error variant"),
        }
    }
    #[test]
    fn test_parse_ethereum_address_invalid_checksum() {
        // Test with an invalid checksum address.
        let address_str = "0x8ba1f109551bd432803012645ac136ddd64dba72";
        let response = parse_ethereum_address(address_str);
        assert_eq!(response, Err(EthereumAddressParseError::ChecksumFailed));
    }
}
