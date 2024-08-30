use comet_bft_rpc::HttpCometBFTRpcClientFactory;
use tendermint_light_client::{
    builder::LightClientBuilder, instance::Instance, light_client::LightClient,
};

pub(crate) struct LightCBFTClientBuilder {
    rpc_client_factory: HttpCometBFTRpcClientFactory,
    light_store: Box<dyn LightStore>,
}

impl LightCBFTClientBuilder {
    pub fn new(
        rpc_client_factory: HttpCometBFTRpcClientFactory,
        light_store: Box<dyn LightStore>,
    ) -> Self {
        Self { rpc_client_factory, light_store }
    }

    pub async fn build(self) -> Instance {
        let rpc_client = self.rpc_client_factory.create_client().unwrap();
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
    }
}
