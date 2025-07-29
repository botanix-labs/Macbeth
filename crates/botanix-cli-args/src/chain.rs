use askama::Template;
use bitcoin::hashes::Hash;
use botanix_authority_edh::{
    extra_data_header::{ExtraDataHeader, CHAIN_VERSION, EXTRA_HEADER_VERSION},
    nums_secp256k1_pk,
};
use botanix_cli_parsers::parsers::SUPPORTED_CHAINS;
use botanix_configs::federation::FederationTomlConfig;
use reth_chainspec::{
    create_botanix_config_with_genesis, BotanixMainnetGenesisConfig, BotanixTestnetGenesisConfig,
    ChainSpec, BOTANIX_MAINNET, BOTANIX_MAINNET_CHAIN_ID, BOTANIX_TESTNET,
    BOTANIX_TESTNET_CHAIN_ID,
};
use reth_primitives::Address;
use std::{fs, path::PathBuf, str::FromStr};

/// The help info for the --chain flag
pub fn chain_help() -> String {
    format!("The chain this node is running.\nPossible values are either a built-in chain or the path to a chain specification file.\n\nBuilt-in chains:\n    {}", SUPPORTED_CHAINS.join(", "))
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
    let (genesis, pegin_conf_depth, chain_id, genesis_hash) = match network {
        BotanixNetwork::Mainnet => {
            let genesis_config = BotanixMainnetGenesisConfig { edh: &edh };
            let rendered_json = genesis_config.render()?;
            let genesis = serde_json::from_str(&rendered_json)?;
            (
                genesis,
                BOTANIX_MAINNET.bitcoin_checkpoint_confirmation_depth,
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
                BOTANIX_TESTNET.bitcoin_checkpoint_confirmation_depth,
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
        lst_fee_receiver,
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
