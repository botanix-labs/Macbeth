use askama::Template;
use bitcoin::hashes::Hash;
use botanix_authority_edh::{
    extra_data_header::{ExtraDataHeader, CHAIN_VERSION, EXTRA_HEADER_VERSION},
    nums_secp256k1_pk,
};
use botanix_chainspec::constants::{
    create_botanix_config_with_genesis, BotanixMainnetGenesisConfig, BotanixTestnetGenesisConfig,
    BOTANIX_MAINNET, BOTANIX_MAINNET_CHAIN_ID, BOTANIX_TESTNET, BOTANIX_TESTNET_CHAIN_ID,
};
use botanix_cli_parsers::parsers::SUPPORTED_CHAINS;
use botanix_configs::federation::FederationTomlConfig;
use reth_chainspec::ChainSpec;
use reth_primitives::Address;
use std::{fs, path::PathBuf, str::FromStr};

/// The help info for the --chain flag
pub fn chain_help() -> String {
    format!("The chain this node is running.\nPossible values are either a built-in chain or the path to a chain specification file.\n\nBuilt-in chains:\n    {}", SUPPORTED_CHAINS.join(", "))
}

/// The Botanix network enum
/// This is used to determine which network to use when creating the chain spec.
#[derive(Debug, Eq, PartialEq)]
pub enum BotanixNetwork {
    /// Mainnet Botanix network
    Mainnet,
    /// Testnet Botanix network
    Testnet,
    /// Devnet Botanix network
    Devnet,
}

impl BotanixNetwork {
    /// Creates a `BotanixNetwork` from command line arguments.
    pub fn from_args(is_testnet: bool, is_devnet: bool) -> eyre::Result<Self> {
        // Validate that only one network argument is passed
        if is_testnet && is_devnet {
            return Err(eyre::eyre!("Both testnet and devnet cannot be enabled at the same time"));
        }

        if is_testnet {
            Ok(Self::Testnet)
        } else if is_devnet {
            Ok(Self::Devnet)
        } else {
            Ok(Self::Mainnet)
        }
    }
    /// Returns `true` if this network is Botanix Mainnet.
    pub const fn is_mainnet(&self) -> bool {
        matches!(self, Self::Mainnet)
    }

    /// Returns `true` if this network is Botanix Testnet.
    pub const fn is_testnet(&self) -> bool {
        matches!(self, Self::Testnet)
    }
    /// Returns `true` if this network is Botanix Devnet.
    pub const fn is_devnet(&self) -> bool {
        matches!(self, Self::Devnet)
    }
}

/// Returns the Botanix network chain spec based on a flag
pub fn get_botanix_chain(raw: &str, is_testnet: bool) -> eyre::Result<ChainSpec> {
    let network = if is_testnet { BotanixNetwork::Testnet } else { BotanixNetwork::Mainnet };

    // our own toml format
    let genesis_toml_config = FederationTomlConfig::from_str(raw)?;
    let botanix_fee_recipient = genesis_toml_config.botanix_fee_recipient;
    let lst_fee_receiver = genesis_toml_config.lst_fee_receiver;

    let extra_data_header = ExtraDataHeader::new(
        EXTRA_HEADER_VERSION,
        CHAIN_VERSION,
        bitcoin::hash_types::BlockHash::all_zeros(),
        nums_secp256k1_pk(),
        Address::ZERO,
    );
    let edh = hex::encode(extra_data_header.serialize());
    let (genesis, pegin_conf_depth, chain_id, genesis_hash, epoch_length) = match network {
        BotanixNetwork::Mainnet => {
            let genesis_config = BotanixMainnetGenesisConfig { edh: &edh };
            let rendered_json = genesis_config.render()?;
            let genesis = serde_json::from_str(&rendered_json)?;
            (
                genesis,
                BOTANIX_MAINNET.bitcoin_checkpoint_confirmation_depth,
                BOTANIX_MAINNET_CHAIN_ID,
                BOTANIX_MAINNET.chainspec().genesis_hash,
                BOTANIX_MAINNET.epoch_length,
            )
        }
        BotanixNetwork::Testnet | BotanixNetwork::Devnet => {
            let genesis_config = BotanixTestnetGenesisConfig { edh: &edh };
            let rendered_json = genesis_config.render()?;
            let genesis = serde_json::from_str(&rendered_json)?;
            (
                genesis,
                BOTANIX_TESTNET.bitcoin_checkpoint_confirmation_depth,
                BOTANIX_TESTNET_CHAIN_ID,
                BOTANIX_TESTNET.chainspec().genesis_hash,
                BOTANIX_TESTNET.epoch_length,
            )
        }
    };
    let botanix_chain = create_botanix_config_with_genesis(
        genesis,
        pegin_conf_depth,
        botanix_fee_recipient,
        chain_id,
        genesis_hash,
        lst_fee_receiver,
        epoch_length,
    );
    Ok(botanix_chain.chainspec().clone())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_args() {
        assert_eq!(BotanixNetwork::from_args(false, false).unwrap(), BotanixNetwork::Mainnet);
        assert_eq!(BotanixNetwork::from_args(true, false).unwrap(), BotanixNetwork::Testnet);
        assert_eq!(BotanixNetwork::from_args(false, true).unwrap(), BotanixNetwork::Devnet);
        assert!(
            BotanixNetwork::from_args(true, true).is_err(),
            "Both testnet and devnet cannot be enabled at the same time"
        );
    }
}
