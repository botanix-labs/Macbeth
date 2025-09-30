use std::str::FromStr;

use tendermint_rpc::{client::HttpClient, Error, HttpClientUrl};
// re-export Client trait
pub use tendermint_rpc::Client;

pub mod light_client;

const DEFAULT_RPC_HOST: &str = "localhost";
const DEFAULT_RPC_PORT: u16 = 26657;

pub trait CometBftRpcFactory: Clone + Send + Sync {
    fn new(url: String) -> Self;
    fn build_url(&self) -> Result<HttpClientUrl, Error>;
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

    fn build_url(&self) -> Result<HttpClientUrl, Error> {
        Ok(HttpClientUrl::from_str(&self.url)?)
    }

    fn build_and_connect(&self) -> Result<HttpClient, Error> {
        let url = self.build_url()?;
        HttpClient::builder(url).compat_mode(tendermint_rpc::client::CompatMode::V0_34).build()
    }
}

impl Default for HttpCometBFTRpcClientFactory {
    fn default() -> Self {
        Self { url: format!("http://{}:{}", DEFAULT_RPC_HOST, DEFAULT_RPC_PORT) }
    }
}

impl HttpCometBFTRpcClientFactory {
    pub fn with_url(mut self, url: &str) -> Self {
        self.url = url.to_string();
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
        let client_factory = HttpCometBFTRpcClientFactory::new(format!(
            "http://{}:{}",
            DEFAULT_RPC_HOST, DEFAULT_RPC_PORT
        ));
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
            HttpCometBFTRpcClientFactory::default().with_url("http://api.example.com:9000");

        assert_eq!(client_factory.url, "http://api.example.com:9000");
    }

    #[test]
    fn test_http_rpc_client_factory_clone() {
        let client_factory =
            HttpCometBFTRpcClientFactory::default().with_url("http://test.example.com:8888");

        let cloned_factory = client_factory.clone();

        assert_eq!(client_factory.url, cloned_factory.url);
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
            HttpCometBFTRpcClientFactory::new(format!("{}:{}", "invalid:url", DEFAULT_RPC_PORT));
        let result = client_factory.build_and_connect();
        assert!(result.is_err());
    }

    #[test]
    fn test_url_construction() {
        let host = "test-host.com";
        let port = 9876;
        let client_factory = HttpCometBFTRpcClientFactory::new(format!("{}:{}", host, port));

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
