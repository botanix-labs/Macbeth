//! Contains RPC handler implementations specific to botanix.

use crate::EthApi;
use reth_provider::BlockReaderIdExt;
use reth_rpc_eth_api::helpers::{botanix::EthBotanixApi, LoadBlock};

impl<Provider, Pool, Network, EvmConfig> EthBotanixApi
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadBlock,
    Provider: BlockReaderIdExt,
{
    #[inline]
    fn provider(&self) -> impl BlockReaderIdExt {
        self.inner.provider()
    }

    fn botanix_provider(&self) -> &reth_rpc_eth_types::builder::botanix_config::Botanix {
        self.inner.botanix_provider()
    }
}
