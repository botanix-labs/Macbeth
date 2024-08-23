use tendermint_rpc::{client::HttpClient, Error};
// re-export Client trait
pub use tendermint_rpc::Client;

const DEFAULT_RPC_URL: &str = "http://localhost:26657";

pub trait CometBftRpcFactory: Clone + Send + Sync {
    fn new(url: String) -> Self;

    fn build_and_connect(&self) -> Result<HttpClient, Error>;
}

#[derive(Clone, Debug)]
pub struct HttpCometBFTRpcClientFactory {
    // storing as String so it works with HttpClient::new()
    // which needs a type that implements try_into()
    url: String,
}

impl CometBftRpcFactory for HttpCometBFTRpcClientFactory {
    fn new(url: String) -> Self {
        Self { url }
    }

    fn build_and_connect(&self) -> Result<HttpClient, Error> {
        HttpClient::new(self.url.as_str())
    }
}

impl Default for HttpCometBFTRpcClientFactory {
    fn default() -> Self {
        Self { url: String::from(DEFAULT_RPC_URL) }
    }
}

mod tests {
    use super::*;

    #[test]
    fn test_http_rpc_client_factory_new() {
        let client_factory = HttpCometBFTRpcClientFactory::new(String::from(DEFAULT_RPC_URL));
        let client = client_factory.build_and_connect();
        assert!(client.is_ok());
    }

    #[test]
    fn test_http_rpc_client_factory_default() {
        let client_factory = HttpCometBFTRpcClientFactory::default();
        let client = client_factory.build_and_connect();
        assert!(client.is_ok());
    }
}
