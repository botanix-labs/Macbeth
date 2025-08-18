use botanix_hardforks::BotanixHardfork;
use reth_chainspec::{ChainSpec, EthChainSpec, EthereumHardfork, Hardforks};
use reth_primitives_traits::constants::EIP1559_INITIAL_BASE_FEE;

use crate::constants::{BOTANIX_INITIAL_BASE_FEE, BOTANIX_TESTNET};
pub mod constants;
pub mod parser;

/// Botanix chain spec type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotanixChainSpec {
    /// [`ChainSpec`].
    pub inner: ChainSpec,

    /// The number of confirmations we require for pegins from the mainchain.
    pub bitcoin_checkpoint_confirmation_depth: u32,

    /// How many checkpoints before the strong confirmation depth to keep (depth < strong)
    /// for validation
    pub historical_bitcoin_checkpoints_count: usize,

    /// How many historical checkpoints to keep (depth > strong) for validation
    pub weak_bitcoin_checkpoints_count: usize,

    /// Block times in seconds
    pub leader_selection_window: Option<u64>,

    /// Botanix fee recipient
    pub botanix_fee_recipient: Option<String>,

    /// LST fee receiver
    /// This is the contract address that receives block fees as part of native staking
    pub lst_fee_receiver: Option<String>,

    /// EIP-225: Clique Proof-of-Authority consensus protocol.
    /// The number of blocks in an epoch for PoA consensus
    pub epoch_length: u64,
}

impl Default for BotanixChainSpec {
    fn default() -> Self {
        Self {
            inner: ChainSpec::default(),
            leader_selection_window: None,
            botanix_fee_recipient: None,
            lst_fee_receiver: None,
            bitcoin_checkpoint_confirmation_depth: 0,
            weak_bitcoin_checkpoints_count: 0,
            historical_bitcoin_checkpoints_count: 0,
            epoch_length: 0,
        }
    }
}

impl BotanixChainSpec {
    pub fn chainspec(&self) -> &ChainSpec {
        &self.inner
    }

    /// Returns the initial base fee based on chain id
    pub fn initial_base_fee_by_chain_id(self) -> u64 {
        if self.chain().id() == BOTANIX_TESTNET.chain().id() {
            BOTANIX_INITIAL_BASE_FEE
        } else {
            EIP1559_INITIAL_BASE_FEE
        }
    }

    /// Get the initial base fee of the genesis block.
    pub fn initial_base_fee(&self) -> Option<u64> {
        // If the base fee is set in the genesis block, we use that instead of the default.
        let genesis_base_fee = self.clone().initial_base_fee_by_chain_id();

        // If London is activated at genesis, we set the initial base fee as per EIP-1559.
        (self.inner.fork(BotanixHardfork::Berlin).active_at_block(0)).then_some(genesis_base_fee)
    }
}

impl EthChainSpec for BotanixChainSpec {
    type Hardfork = EthereumHardfork;

    fn chain(&self) -> alloy_chains::Chain {
        self.inner.chain()
    }

    fn activation_condition(&self, hardfork: Self::Hardfork) -> reth_chainspec::ForkCondition {
        self.inner.activation_condition(hardfork)
    }
}

impl Hardforks for BotanixChainSpec {
    fn fork<H: reth_chainspec::Hardfork>(&self, fork: H) -> reth_chainspec::ForkCondition {
        self.inner.fork(fork)
    }

    fn forks_iter(
        &self,
    ) -> impl Iterator<Item = (&dyn reth_chainspec::Hardfork, reth_chainspec::ForkCondition)> {
        self.inner.forks_iter()
    }
}

impl From<ChainSpec> for BotanixChainSpec {
    fn from(value: ChainSpec) -> Self {
        Self { inner: value, ..Default::default() }
    }
}
