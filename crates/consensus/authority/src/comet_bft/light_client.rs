use comet_bft_rpc::{CometBftRpcFactory, HttpCometBFTRpcClientFactory};
use tendermint_light_client::{
    builder::LightClientBuilder,
    instance::Instance,
    light_client::LightClient,
    store::{memory::MemoryStore, LightStore},
};

/// Builds a light client from a comet bft rpc client factory and a light store
pub(crate) struct LightCBFTClientBuilder {
    /// The rpc client factory
    rpc_client_factory: HttpCometBFTRpcClientFactory,
    /// The light store, best to use [MemoryStore] from the tendermint_light_client crate
    light_store: Box<dyn LightStore>,
}

impl LightCBFTClientBuilder {
    pub fn new(rpc_client_factory: HttpCometBFTRpcClientFactory) -> Self {
        let light_store = Box::new(MemoryStore::new());

        Self { rpc_client_factory, light_store }
    }

    pub fn with_light_store(mut self, light_store: Box<dyn LightStore>) -> Self {
        self.light_store = light_store;
        self
    }

    pub async fn build(self) -> Instance {
        let rpc_client =
            self.rpc_client_factory.build_and_connect().expect("should connect to RPC client");
        let node_id = rpc_client.status().await.expect("Failed to get node info").node_info.id;
        let light_client = LightClientBuilder::prod(
            node_id,
            rpc_client,
            self.light_store,
            Options::default(),
            None,
        );

        let light = light_client.trust_from_store().unwrap().build();
        light
            .light_client
            .verify_to_highest(&mut light.state)
            .expect("Failed to verify light client");

        light
    }
}
