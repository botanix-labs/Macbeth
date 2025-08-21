use crate::errors::{EthereumAddressParseError, UrlParsingError};
use alloy_primitives::{hex, Address};
use askama::Template;
use bitcoin::hashes::Hash;
use botanix_authority_edh::{
    extra_data_header::{ExtraDataHeader, CHAIN_VERSION, EXTRA_HEADER_VERSION},
    nums_secp256k1_pk,
};
use botanix_chainspec::constants::{
    create_botanix_config_with_genesis, BotanixTestnetGenesisConfig, BOTANIX_MAINNET,
    BOTANIX_TESTNET, BOTANIX_TESTNET_CHAIN_ID,
};
use botanix_configs::federation::FederationTomlConfig;
use reth_chainspec::{ChainSpec, DEV, HOLESKY, MAINNET, SEPOLIA};
use reth_cli_util::parsers::SocketAddressParsingError;
use reth_primitives::Genesis;
use std::{fs, path::PathBuf, str::FromStr, sync::Arc};
use url::Url;

/// Chains supported by reth. First value should be used as the default.
pub const SUPPORTED_CHAINS: &[&str] =
    &["mainnet", "sepolia", "holesky", "dev", "botanix_testnet", "botanix_mainnet"];

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

/// Clap value parser for [`ChainSpec`]s.
///
/// The value parser matches either a known chain, the path
/// to a json file, or a json formatted string in-memory. The json can be either
/// a serialized [`ChainSpec`] or Genesis struct.
pub fn genesis_value_parser(s: &str) -> eyre::Result<Arc<ChainSpec>, eyre::Error> {
    Ok(match s {
        "mainnet" => MAINNET.clone(),
        "sepolia" => SEPOLIA.clone(),
        "holesky" => HOLESKY.clone(),
        "dev" => DEV.clone(),
        "botanix_testnet" | "botanix-testnet" => BOTANIX_TESTNET.inner_arc(),
        "botanix_mainnet" | "botanix-mainnet" => BOTANIX_MAINNET.inner_arc(),
        _ => {
            // try to read json from path first
            let raw = match fs::read_to_string(PathBuf::from(shellexpand::full(s)?.into_owned())) {
                Ok(raw) => raw,
                Err(io_err) => {
                    // valid json may start with "\n", but must contain "{"
                    if s.contains('{') {
                        s.to_string()
                    } else {
                        return Err(io_err.into()); // assume invalid path
                    }
                }
            };
            // both serialized Genesis and ChainSpec supported
            let genesis_toml_config = FederationTomlConfig::from_str(&raw)?;
            let botanix_fee_recipient = genesis_toml_config.botanix_fee_recipient.clone();
            let lst_fee_receiver = genesis_toml_config.lst_fee_receiver;

            let _public_keys = genesis_toml_config
                .federation_member_public_key
                .iter()
                .map(|key| {
                    secp256k1::PublicKey::from_str(&key.key)
                        .expect("Invalid hex string for PublicKey")
                })
                .collect::<Vec<secp256k1::PublicKey>>();

            let extra_data_header = ExtraDataHeader::new(
                EXTRA_HEADER_VERSION,
                CHAIN_VERSION,
                bitcoin::hash_types::BlockHash::all_zeros(),
                // Agg key in genesis should always be NUMS point for genesis block
                nums_secp256k1_pk(),
                Address::ZERO,
            );
            let edh = hex::encode(extra_data_header.serialize());
            let botanix_testnet_config_genesis = BotanixTestnetGenesisConfig { edh: &edh };
            let rendered_json = botanix_testnet_config_genesis.render()?;
            let genesis = serde_json::from_str(&rendered_json)?;
            let botanix_testnet = create_botanix_config_with_genesis(
                genesis,
                BOTANIX_TESTNET.bitcoin_checkpoint_confirmation_depth,
                botanix_fee_recipient,
                BOTANIX_TESTNET_CHAIN_ID,
                BOTANIX_TESTNET.inner().genesis_hash,
                lst_fee_receiver,
                BOTANIX_TESTNET.epoch_length,
            );
            botanix_testnet.inner_arc()
        }
    })
}

/// Clap value parser for [`ChainSpec`]s.
///
/// The value parser matches either a known chain, the path
/// to a json file, or a json formatted string in-memory. The json needs to be a Genesis struct.
pub fn chain_value_parser(s: &str) -> eyre::Result<Arc<ChainSpec>, eyre::Error> {
    Ok(match s {
        "mainnet" => MAINNET.clone(),
        "sepolia" => SEPOLIA.clone(),
        "holesky" => HOLESKY.clone(),
        "dev" => DEV.clone(),
        "botanix_testnet" | "botanix-testnet" => BOTANIX_TESTNET.inner_arc(),
        "botanix_mainnet" | "botanix-mainnet" => BOTANIX_MAINNET.inner_arc(),
        _ => {
            // try to read json from path first
            let raw = match fs::read_to_string(PathBuf::from(shellexpand::full(s)?.into_owned())) {
                Ok(raw) => raw,
                Err(io_err) => {
                    // valid json may start with "\n", but must contain "{"
                    if s.contains('{') {
                        s.to_string()
                    } else {
                        return Err(io_err.into()); // assume invalid path
                    }
                }
            };

            // both serialized Genesis and ChainSpec structs supported
            let genesis: Genesis = serde_json::from_str(&raw)?;

            Arc::new(genesis.into())
        }
    })
}

#[cfg(test)]
mod tests {
    use reth_primitives::{hex, Address, ChainConfig, Genesis, GenesisAccount, B256, U256};
    use std::collections::HashMap;

    use crate::{
        errors::EthereumAddressParseError,
        parsers::{
            chain_value_parser, genesis_value_parser, parse_ethereum_address, SUPPORTED_CHAINS,
        },
    };

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

    #[test]
    fn parse_known_chain_spec() {
        for chain in SUPPORTED_CHAINS {
            chain_value_parser(chain).unwrap();
            genesis_value_parser(chain).unwrap();
        }
    }
}
