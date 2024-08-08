use crate::bitcoind::{BitcoindError, BitcoindFactory, RpcApiExt};
use bitcoincore_rpc::{jsonrpc::serde_json, Error as JsonRPCError};

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

impl RpcApiExt for MockBitcoind {
    async fn is_synced(&self) -> Result<bool, BitcoindError> {
        Ok(true)
    }

    async fn wait_until_synced(&self) {}
}

#[derive(Debug, Clone)]
pub struct MockBitcoindFactory;
impl BitcoindFactory for MockBitcoindFactory {
    fn new(_config: crate::bitcoind::BitcoindConfig) -> Self {
        Self {}
    }

    fn build_and_connect(&self) -> Result<impl RpcApiExt, JsonRPCError> {
        Ok(MockBitcoind::new())
    }
}
