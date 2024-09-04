use tendermint_rpc::{client::HttpClient, Error};

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

