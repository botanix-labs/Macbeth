//! Loads a pending block from database. Helper trait for `eth_` block, transaction, call and trace
//! RPC methods.

use futures::Future;
use reth_chainspec::ChainSpec;
use reth_primitives::U256;
use reth_provider::{BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, StateProviderFactory};
use reth_rpc_eth_types::{builder::botanix_config::Botanix, EthApiError};
use revm_primitives::Address;

use crate::EthApiTypes;

/// Loads a pending block from database.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` blocks RPC methods.
pub trait EthBotanixApi: EthApiTypes {
    /// Returns a handle for reading data from disk.
    ///
    /// Data access in default (L1) trait method implementations.
    fn provider(
        &self,
    ) -> impl BlockReaderIdExt
           + EvmEnvProvider
           + ChainSpecProvider<ChainSpec = ChainSpec>
           + StateProviderFactory;

    /// Returns a handle to the botanix provider
    fn botanix_provider(&self) -> &Botanix;

    /// Retrieves the gateway address for deposits
    fn get_gateway_address(
        &self,
        eth_address: Address,
    ) -> impl Future<Output = Result<Option<(bitcoin::Address, secp256k1::PublicKey)>, Self::Error>> + Send
    {
        async move {
            let pegin_info = self
                .botanix_provider()
                .get_gateway_address(eth_address)
                .await
                .map_err(|_| EthApiError::GatewayAddress)?;
            Ok(Some(pegin_info))
        }
    }

    /// Retrieves the merkle proof from the db
    fn get_merkle_proof(
        &self,
        txid: String,
        block_hash: String,
    ) -> impl Future<Output = Result<Vec<u8>, Self::Error>> + Send {
        async move {
            let pegin_info = self
                .botanix_provider()
                .get_merkle_proof(txid, block_hash)
                .await
                .map_err(|_| EthApiError::GetMerkleProof)?;
            Ok(pegin_info)
        }
    }

    /// Retrieves the btc fee rate
    fn get_btc_fee_rate(&self) -> impl Future<Output = Result<U256, Self::Error>> + Send {
        async move {
            let fee_rate = self
                .botanix_provider()
                .get_btc_fee_rate()
                .await
                .map_err(|_| EthApiError::GetBtcFee)?;
            Ok(fee_rate)
        }
    }
}
