use std::str::FromStr;

use tendermint_rpc::{client::HttpClient, Error, HttpClientUrl};
// re-export Client trait
pub use tendermint_rpc::Client;

pub mod light_client;

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
        let url = HttpClientUrl::from_str(format!("http://{}:{}", self.host, self.port).as_str())?;
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use tendermint_rpc::{HttpClientUrl, Url};

    use crate::{
        CometBftRpcFactory, HttpCometBFTRpcClientFactory, DEFAULT_RPC_HOST, DEFAULT_RPC_PORT,
    };

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

    #[test]
    fn test_http_rpc_client_factory_chained_methods() {
        let client_factory =
            HttpCometBFTRpcClientFactory::default().with_host("api.example.com").with_port(9000);

        assert_eq!(client_factory.host, "api.example.com");
        assert_eq!(client_factory.port, 9000);
    }

    #[test]
    fn test_http_rpc_client_factory_clone() {
        let client_factory =
            HttpCometBFTRpcClientFactory::default().with_host("test.example.com").with_port(8888);

        let cloned_factory = client_factory.clone();

        assert_eq!(client_factory.host, cloned_factory.host);
        assert_eq!(client_factory.port, cloned_factory.port);
    }

    #[test]
    fn test_http_rpc_client_factory_debug() {
        let client_factory = HttpCometBFTRpcClientFactory::default();
        let debug_output = format!("{:?}", client_factory);

        assert!(debug_output.contains(DEFAULT_RPC_HOST));
        assert!(debug_output.contains(&DEFAULT_RPC_PORT.to_string()));
    }

    #[test]
    fn test_invalid_url_format() {
        let client_factory =
            HttpCometBFTRpcClientFactory::new("invalid:url".to_string(), DEFAULT_RPC_PORT);
        let result = client_factory.build_and_connect();
        assert!(result.is_err());
    }

    #[test]
    fn test_url_construction() {
        let host = "test-host.com";
        let port = 9876;
        let client_factory = HttpCometBFTRpcClientFactory::new(host.to_string(), port);

        let expected_url_str = format!("http://{}:{}", host, port);
        let expected_url: Url = HttpClientUrl::from_str(&expected_url_str).unwrap().into();
        assert_eq!(host, expected_url.host());
        assert_eq!(port, expected_url.port());

        let client_result = client_factory.build_and_connect();
        if client_result.is_ok() {
            assert!(true);
        } else {
            let error = client_result.unwrap_err();
            println!("{:?}", error);
            let is_connection_error = matches!(error, tendermint_rpc::Error(__, _));
            assert!(is_connection_error);
        }
    }
}
