use crate::bitcoind::BitcoindFactory;
use bitcoincore_rpc::{
    json::{EstimateMode, EstimateSmartFeeResult, GetBlockHeaderResult},
    jsonrpc::serde_json,
    Auth, Client, Error as JsonRPCError, RpcApi,
};

pub struct MockBitcoind;
impl bitcoincore_rpc::RpcApi for MockBitcoind {
    fn call<T: for<'a> serde::de::Deserialize<'a>>(
        &self,
        _method: &str,
        _params: &[serde_json::Value],
    ) -> Result<T, bitcoincore_rpc::Error> {
        unimplemented!()
    }
}

impl MockBitcoind {
    pub fn new() -> Self {
        Self {}
    }
}

#[derive(Debug, Clone)]
pub struct MockBitcoindFactory;
impl BitcoindFactory for MockBitcoindFactory {
    fn new(_config: crate::bitcoind::BitcoindConfig) -> Self {
        Self {}
    }

    fn build_and_connect(&self) -> Result<impl RpcApi, JsonRPCError> {
        Ok(MockBitcoind::new())
    }
}
