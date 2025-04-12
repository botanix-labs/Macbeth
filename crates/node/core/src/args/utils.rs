//! Clap parser utilities

use alloy_genesis::Genesis;
use askama::Template;
use bitcoin::hashes::Hash;
use reth_chainspec::{
    create_botanix_config_with_genesis, BotanixMainnetGenesisConfig, BotanixTestnetGenesisConfig,
    ChainSpec, BOTANIX_MAINNET, BOTANIX_MAINNET_CHAIN_ID, BOTANIX_TESTNET,
    BOTANIX_TESTNET_CHAIN_ID, DEV,
};
use reth_fs_util as fs;
use reth_primitives::{
    constants::nums_secp256k1_pk,
    extra_data_header::{ExtraDataHeader, CHAIN_VERSION, EXTRA_HEADER_VERSION},
    Address,
};
use std::{path::PathBuf, str::FromStr, sync::Arc};
use tracing::info;

use reth_chainspec::{HOLESKY, MAINNET, SEPOLIA};

use super::FederationTomlConfig;

/// Chains supported by reth. First value should be used as the default.
pub const SUPPORTED_CHAINS: &[&str] =
    &["mainnet", "sepolia", "holesky", "dev", "botanix_testnet", "botanix_mainnet"];

/// The help info for the --chain flag
pub fn chain_help() -> String {
    format!("The chain this node is running.\nPossible values are either a built-in chain or the path to a chain specification file.\n\nBuilt-in chains:\n    {}", SUPPORTED_CHAINS.join(", "))
}

/// Load the federation setup toml
pub fn load_federation_config_toml(path: &PathBuf) -> eyre::Result<FederationTomlConfig> {
    let _ = fs::metadata(path)?;
    let raw = fs::read_to_string(path)?;
    let genesis_toml_config = FederationTomlConfig::from_str(&raw)?;
    Ok(genesis_toml_config)
}

/// The Botanix network enum
/// This is used to determine which network to use when creating the chain spec.
#[derive(Debug)]
pub enum BotanixNetwork {
    /// Mainnet Botanix network
    Mainnet,
    /// Testnet Botanix network
    Testnet,
}

/// Returns the Botanix network chain spec based on a flag
pub fn get_botanix_chain(raw: &str, is_testnet: bool) -> eyre::Result<ChainSpec> {
    let network = if is_testnet { BotanixNetwork::Testnet } else { BotanixNetwork::Mainnet };
    info!("Creating botanix chain spec for: {:?}", network);

    // our own toml format
    let genesis_toml_config = FederationTomlConfig::from_str(raw)?;
    let botanix_fee_recipient = genesis_toml_config.botanix_fee_recipient;
    info!("Botanix fee recipient: {:?}", botanix_fee_recipient);

    let extra_data_header = ExtraDataHeader::new(
        EXTRA_HEADER_VERSION,
        CHAIN_VERSION,
        bitcoin::hash_types::BlockHash::all_zeros(),
        nums_secp256k1_pk(),
        Address::ZERO,
    );
    let edh = hex::encode(extra_data_header.serialize());
    let (genesis, pegin_conf_depth, chain_id, genesis_hash) = match network {
        BotanixNetwork::Mainnet => {
            let genesis_config = BotanixMainnetGenesisConfig { edh: &edh };
            let rendered_json = genesis_config.render()?;
            let genesis = serde_json::from_str(&rendered_json)?;
            (
                genesis,
                BOTANIX_MAINNET.parent_confirmation_depth,
                BOTANIX_MAINNET_CHAIN_ID,
                BOTANIX_MAINNET.genesis_hash,
            )
        }
        BotanixNetwork::Testnet => {
            let genesis_config = BotanixTestnetGenesisConfig { edh: &edh };
            let rendered_json = genesis_config.render()?;
            let genesis = serde_json::from_str(&rendered_json)?;
            (
                genesis,
                BOTANIX_TESTNET.parent_confirmation_depth,
                BOTANIX_TESTNET_CHAIN_ID,
                BOTANIX_TESTNET.genesis_hash,
            )
        }
    };
    let botanix_chain = create_botanix_config_with_genesis(
        genesis,
        pegin_conf_depth,
        botanix_fee_recipient,
        chain_id,
        genesis_hash,
    );
    Ok(botanix_chain)
}

/// Returns the botanix network chain spec using the config at the passed path
pub fn get_chain_from_federation_config(
    s: &str,
    is_testnet: bool,
) -> eyre::Result<ChainSpec, eyre::Error> {
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

    get_botanix_chain(&raw, is_testnet)
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
                BOTANIX_TESTNET.parent_confirmation_depth,
                botanix_fee_recipient,
                BOTANIX_TESTNET_CHAIN_ID,
                BOTANIX_TESTNET.genesis_hash,
            );
            Arc::new(botanix_testnet)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_chain_spec() {
        for chain in SUPPORTED_CHAINS {
            chain_value_parser(chain).unwrap();
        }
    }
}
