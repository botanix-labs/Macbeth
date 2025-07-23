use crate::errors::{EthereumAddressParseError, UrlParsingError};
use alloy_primitives::{hex, Address};
use reth_cli_util::parsers::SocketAddressParsingError;
use url::Url;

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

/// Parse a [URL] from a `str` value
pub fn parse_url(value: &str) -> eyre::Result<Url, UrlParsingError> {
    let url = Url::parse(value).map_err(|_e| UrlParsingError::Parse(value.to_owned()))?;
    Ok(url)
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

#[cfg(test)]
mod tests {
    use super::*;

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
