use crate::{
    database::StateProviderDatabase,
    processor::EVMProcessor,
    stack::{InspectorStack, InspectorStackConfig},
};
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_evm::ConfigureEvm;
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::ChainSpec;
use reth_provider::{ExecutorFactory, PrunableBlockExecutor, StateProvider};
use std::sync::Arc;

/// Factory for creating [EVMProcessor].
#[derive(Clone, Debug)]
pub struct EvmProcessorFactory<EvmConfig, BF> {
    chain_spec: Arc<ChainSpec>,
    stack: Option<InspectorStack>,
    /// Type that defines how the produced EVM should be configured.
    evm_config: EvmConfig,
    /// Factory for creating bitcoind clients + Bitcoin network
    /// leaving as optional for executions that do not require this
    /// For any Botanix uses this needs to be defined during the creation of the node components
    bitcoin_resource: Option<(BF, bitcoin::Network)>,
}

impl<EvmConfig: ConfigureEvm, BF> EvmProcessorFactory<EvmConfig, BF> {
    /// Create new factory
    pub fn new(chain_spec: Arc<ChainSpec>, evm_config: EvmConfig) -> Self {
        Self { chain_spec, stack: None, evm_config, bitcoin_resource: None }
    }

    /// Set the bitcoind factory and network for the factory
    pub fn with_bitcoind_factory(
        mut self,
        bitcoind_factory: BF,
        network: bitcoin::Network,
    ) -> Self {
        self.bitcoin_resource = Some((bitcoind_factory, network));
        self
    }

    /// Sets the inspector stack for all generated executors.
    pub fn with_stack(mut self, stack: InspectorStack) -> Self {
        self.stack = Some(stack);
        self
    }

    /// Sets the inspector stack for all generated executors using the provided config.
    pub fn with_stack_config(mut self, config: InspectorStackConfig) -> Self {
        self.stack = Some(InspectorStack::new(config));
        self
    }
}

impl<EvmConfig, BF> ExecutorFactory for EvmProcessorFactory<EvmConfig, BF>
where
    EvmConfig: ConfigureEvm + Send + Sync + Clone + 'static,
    BF: BitcoindFactory + Send + Sync + Clone + 'static,
{
    fn with_state<'a, SP: StateProvider + 'a>(
        &'a self,
        sp: SP,
    ) -> Box<dyn PrunableBlockExecutor<Error = BlockExecutionError> + 'a> {
        let database_state = StateProviderDatabase::new(sp);
        let mut evm = EVMProcessor::new_with_db(
            self.chain_spec.clone(),
            database_state,
            self.evm_config.clone(),
        );
        if let Some(stack) = &self.stack {
            evm.set_stack(stack.clone());
        }
        if let Some((bitcoind_factory, network)) = &self.bitcoin_resource {
            evm.with_bitcoind_factory(bitcoind_factory.clone(), network.clone());
        }
        Box::new(evm)
    }
}
