//! Contains RPC handler implementations specific to botanix.

use crate::EthApi;
use botanix_rpc_client::botanix::EthBotanixApi;
use botanix_rpc_config::botanix_config::Botanix;
use reth_provider::BlockReaderIdExt;
use reth_rpc_eth_api::helpers::LoadBlock;

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

    fn botanix_provider(&self) -> &Botanix {
        self.inner.botanix_provider()
    }
}
