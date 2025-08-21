use alloy_chains::Chain;
use alloy_eips::eip1559::ETHEREUM_BLOCK_GAS_LIMIT;
use alloy_genesis::Genesis;
use alloy_primitives::{b256, B256, U256};
use askama::Template;
use botanix_hardforks::BotanixHardfork;
use once_cell::sync::Lazy;
use reth_chainspec::{BaseFeeParams, BaseFeeParamsKind, ChainSpec, EthereumHardfork};
use std::sync::Arc;

use crate::BotanixChainSpec;

/// Botanix Mainnet genesis hash:
/// `0x0210ae550e730d0e18f96896b80caad6f59dcc0b83b67421975716d155d027c6`
pub const BOTANIX_MAINNET_GENESIS: B256 =
    b256!("0210ae550e730d0e18f96896b80caad6f59dcc0b83b67421975716d155d027c6");

/// Botanix Testnet genesis hash.
pub const BOTANIX_TESTNET_GENESIS: B256 =
    b256!("3797638175875c37cefa72ef546db685e43c81ba4af8238b48a495f98d61588d");

/// factor of 1000 less than the initial base fee of 1 gwei suggested in EIP-1559
pub const BOTANIX_INITIAL_BASE_FEE: u64 = 1_000_000;

/// The Botanix specs
///
/// Includes Testnet and Mainnet
pub const BOTANIX_TESTNET_CHAIN_ID: u64 = 3636;
/// Mainnet chain id
pub const BOTANIX_MAINNET_CHAIN_ID: u64 = 3637;

/// Botanix Testnet Genesis Configuration
#[derive(Template, Clone, Debug)]
#[template(path = "botanix_testnet_template.json", ext = "json", escape = "none")]
pub struct BotanixTestnetGenesisConfig<'a> {
    /// Extra data header field
    pub edh: &'a str,
}

/// Botanix Mainnet Genesis Configuration
#[derive(Template, Clone, Debug)]
#[template(path = "botanix_mainnet_template.json", ext = "json", escape = "none")]
pub struct BotanixMainnetGenesisConfig<'a> {
    /// Extra data header field
    pub edh: &'a str,
}

/// The Botanix Testnet
pub static BOTANIX_TESTNET: Lazy<Arc<BotanixChainSpec>> = Lazy::new(|| {
    let mut spec = ChainSpec {
        chain: Chain::from_id(BOTANIX_TESTNET_CHAIN_ID),
        genesis: serde_json::from_str(include_str!("../genesis/botanix_testnet.json"))
            .expect("Can't deserialize Botanix Testnet genesis json"),
        genesis_hash: Some(BOTANIX_TESTNET_GENESIS),
        paris_block_and_final_difficulty: Some((0, U256::from(0))),
        hardforks: BotanixHardfork::botanix()
            .map(|(bot_hf, fk)| (EthereumHardfork::from(bot_hf), fk))
            .into(),
        deposit_contract: None, // only relevant for PoS chains
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        max_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
        prune_delete_limit: 20000,
    };
    spec.genesis.config.dao_fork_support = false;
    let botanix_spec = BotanixChainSpec {
        inner: spec,
        bitcoin_checkpoint_confirmation_depth: 1,
        weak_bitcoin_checkpoints_count: 0,
        historical_bitcoin_checkpoints_count: 1,
        leader_selection_window: Some(20),
        botanix_fee_recipient: None,
        lst_fee_receiver: None,
        epoch_length: 10,
    };
    botanix_spec.into()
});

/// The Botanix Mainnet
pub static BOTANIX_MAINNET: Lazy<Arc<BotanixChainSpec>> = Lazy::new(|| {
    let mut spec = ChainSpec {
        chain: Chain::from_id(BOTANIX_MAINNET_CHAIN_ID),
        genesis: serde_json::from_str(include_str!("../genesis/botanix_mainnet.json"))
            .expect("Can't deserialize Botanix Mainnet genesis json"),
        genesis_hash: Some(BOTANIX_MAINNET_GENESIS),
        paris_block_and_final_difficulty: Some((0, U256::from(0))),
        hardforks: BotanixHardfork::botanix()
            .map(|(bot_hf, fk)| (EthereumHardfork::from(bot_hf), fk))
            .into(),
        deposit_contract: None, // only relevant for PoS chains
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        max_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
        prune_delete_limit: 20000,
    };
    spec.genesis.config.dao_fork_support = false;
    let botanix_spec = BotanixChainSpec {
        inner: spec,
        bitcoin_checkpoint_confirmation_depth: 18,
        weak_bitcoin_checkpoints_count: 1,
        historical_bitcoin_checkpoints_count: 1,
        leader_selection_window: Some(20),
        botanix_fee_recipient: None,
        lst_fee_receiver: None,
        epoch_length: 100,
    };
    botanix_spec.into()
});

/// Creates a new botanix chain spec using a custom genesis block
pub fn create_botanix_config_with_genesis(
    genesis: Genesis,
    pegin_conf_depth: u32,
    botanix_fee_recipient: String,
    chain_id: u64,
    genesis_hash: Option<B256>,
    lst_fee_receiver: String,
    epoch_length: u64,
) -> BotanixChainSpec {
    let chainspec = ChainSpec {
        chain: Chain::from_id(chain_id),
        genesis,
        genesis_hash,
        paris_block_and_final_difficulty: Some((0, U256::from(0))),
        hardforks: BotanixHardfork::botanix()
            .map(|(bot_hf, fk)| (EthereumHardfork::from(bot_hf), fk))
            .into(),
        deposit_contract: None, // Only relevant for PoS chains
        max_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
        prune_delete_limit: 1700,
        ..Default::default()
    };
    let botanix_spec = BotanixChainSpec {
        inner: chainspec,
        bitcoin_checkpoint_confirmation_depth: pegin_conf_depth,
        weak_bitcoin_checkpoints_count: 0,
        historical_bitcoin_checkpoints_count: 1,
        leader_selection_window: Some(20),
        botanix_fee_recipient: Some(botanix_fee_recipient),
        lst_fee_receiver: Some(lst_fee_receiver),
        epoch_length,
    };
    botanix_spec
}
