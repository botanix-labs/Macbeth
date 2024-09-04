use std::str::FromStr;

use tendermint_rpc::{client::HttpClient, Error, HttpClientUrl};
// re-export Client trait
pub use tendermint_rpc::Client;

const DEFAULT_RPC_HOST: &str = "localhost";
const DEFAULT_RPC_PORT: u16 = 26657;

pub trait CometBftRpcFactory: Clone + Send + Sync {
    fn new(host: String, port: u16) -> Self;

    fn build_and_connect(&self) -> Result<HttpClient, Error>;
}

#[derive(Clone, Debug)]
pub struct HttpCometBFTRpcClientFactory {
    // storing as String so it works with HttpClient::new()
    // which needs a type that implements try_into()
    host: String,
    port: u16,
}

impl CometBftRpcFactory for HttpCometBFTRpcClientFactory {
    fn new(host: String, port: u16) -> Self {
        Self { host, port }
    }

    fn build_and_connect(&self) -> Result<HttpClient, Error> {
        let url = HttpClientUrl::from_str(
            format!("http://{}:{}", self.host, self.port.to_string()).as_str(),
        )?;
        HttpClient::builder(url).compat_mode(tendermint_rpc::client::CompatMode::V0_34).build()
    }
}

impl Default for HttpCometBFTRpcClientFactory {
    fn default() -> Self {
        Self { host: DEFAULT_RPC_HOST.to_string(), port: DEFAULT_RPC_PORT }
    }
}

impl HttpCometBFTRpcClientFactory {
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    pub fn with_host(mut self, host: &str) -> Self {
        self.host = host.to_string();
        self
    }
}

mod tests {
    use super::*;

    #[test]
    fn test_http_rpc_client_factory_new() {
        let client_factory =
            HttpCometBFTRpcClientFactory::new(DEFAULT_RPC_HOST.to_string(), DEFAULT_RPC_PORT);
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
