use crate::errors::{EthereumAddressParseError, UrlParsingError};
use alloy_primitives::{hex, Address};
use askama::Template;
use bitcoin::hashes::Hash;
use botanix_authority_edh::extra_data_header::{
    ExtraDataHeader, CHAIN_VERSION, EXTRA_HEADER_VERSION,
};
use botanix_configs::federation::FederationTomlConfig;
use reth_chainspec::{
    create_botanix_config_with_genesis, BotanixTestnetGenesisConfig, ChainSpec, BOTANIX_MAINNET,
    BOTANIX_TESTNET, BOTANIX_TESTNET_CHAIN_ID, DEV, HOLESKY, MAINNET, SEPOLIA,
};
use reth_cli_util::parsers::SocketAddressParsingError;
use reth_primitives::{constants::nums_secp256k1_pk, Genesis};
use std::{fs, path::PathBuf, str::FromStr, sync::Arc};
use url::Url;

/// Chains supported by reth. First value should be used as the default.
pub const SUPPORTED_CHAINS: &[&str] =
    &["mainnet", "sepolia", "holesky", "goerli", "dev", "botanix_testnet", "botanix_mainnet"];

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
        "botanix_testnet" | "botanix-testnet" => BOTANIX_TESTNET.clone(),
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
            // our own toml format
            let genesis_toml_config = FederationTomlConfig::from_str(&raw)?;
            let botanix_fee_recipient = genesis_toml_config.botanix_fee_recipient;
            let lst_fee_receiver = genesis_toml_config.lst_fee_receiver;

            let extra_data_header = ExtraDataHeader::new(
                EXTRA_HEADER_VERSION,
                CHAIN_VERSION,
                bitcoin::hash_types::BlockHash::all_zeros(),
                // Agg key in genesis should always be NUMS point
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
                BOTANIX_TESTNET.genesis_hash,
                lst_fee_receiver,
            );
            Arc::new(botanix_testnet)
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
        "botanix_testnet" | "botanix-testnet" => BOTANIX_TESTNET.clone(),
        "botanix_mainnet" | "botanix-mainnet" => BOTANIX_MAINNET.clone(),
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
    use reth_chainspec::ChainSpecBuilder;
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
            // chain_spec_value_parser(chain).unwrap();
            genesis_value_parser(chain).unwrap();
        }
    }

    #[test]
    fn parse_chain_spec_from_memory() {
        let custom_genesis_from_json = r#"
{
    "nonce": "0x0",
    "timestamp": "0x653FEE9E",
    "gasLimit": "0x1388",
    "difficulty": "0x0",
    "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "coinbase": "0x0000000000000000000000000000000000000000",
    "alloc": {
        "0x6Be02d1d3665660d22FF9624b7BE0551ee1Ac91b": {
            "balance": "0x21"
        }
    },
    "number": "0x0",
    "gasUsed": "0x0",
    "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "config": {
        "chainId": 2600,
        "homesteadBlock": 0,
        "eip150Block": 0,
        "eip155Block": 0,
        "eip158Block": 0,
        "byzantiumBlock": 0,
        "constantinopleBlock": 0,
        "petersburgBlock": 0,
        "istanbulBlock": 0,
        "berlinBlock": 0,
        "londonBlock": 0,
        "terminalTotalDifficulty": 0,
        "terminalTotalDifficultyPassed": true,
        "shanghaiTime": 0
    }
}
"#;

        let chain_from_json = genesis_value_parser(custom_genesis_from_json).unwrap();

        // using structs
        let config = ChainConfig {
            chain_id: 2600,
            homestead_block: Some(0),
            eip150_block: Some(0),
            eip155_block: Some(0),
            eip158_block: Some(0),
            byzantium_block: Some(0),
            constantinople_block: Some(0),
            petersburg_block: Some(0),
            istanbul_block: Some(0),
            berlin_block: Some(0),
            london_block: Some(0),
            shanghai_time: Some(0),
            terminal_total_difficulty: Some(U256::ZERO),
            terminal_total_difficulty_passed: true,
            ..Default::default()
        };
        let genesis = Genesis {
            config,
            nonce: 0,
            timestamp: 1698688670,
            gas_limit: 5000,
            difficulty: U256::ZERO,
            mix_hash: B256::ZERO,
            coinbase: Address::ZERO,
            number: Some(0),
            ..Default::default()
        };

        // seed accounts after genesis struct created
        let address = hex!("6Be02d1d3665660d22FF9624b7BE0551ee1Ac91b").into();
        let account = GenesisAccount::default().with_balance(U256::from(33));
        let genesis = genesis.extend_accounts(HashMap::from([(address, account)]));

        let custom_genesis_from_struct = serde_json::to_string(&genesis).unwrap();
        let chain_from_struct = genesis_value_parser(&custom_genesis_from_struct).unwrap();
        assert_eq!(chain_from_json.genesis(), chain_from_struct.genesis());
    }
}
