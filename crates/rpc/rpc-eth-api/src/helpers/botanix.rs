//! Loads a pending block from database. Helper trait for `eth_` block, transaction, call and trace
//! RPC methods.

use std::time::{Duration, Instant};

use futures::Future;
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_evm::{system_calls::pre_block_beacon_root_contract_call, ConfigureEvm, ConfigureEvmEnv};
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{
    constants::{eip4844::MAX_DATA_GAS_PER_BLOCK, BEACON_NONCE, EMPTY_ROOT_HASH},
    proofs::calculate_transaction_root,
    revm_primitives::{
        BlockEnv, CfgEnv, CfgEnvWithHandlerCfg, EVMError, Env, ExecutionResult, InvalidTransaction,
        ResultAndState, SpecId,
    },
    Block, BlockNumber, Header, IntoRecoveredTransaction, Receipt, Requests,
    SealedBlockWithSenders, SealedHeader, TransactionSignedEcRecovered, B256,
    EMPTY_OMMER_ROOT_HASH, U256,
};
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ProviderError,
    StateProviderFactory,
};
use reth_revm::{
    database::StateProviderDatabase, state_change::post_block_withdrawals_balance_increments,
};
use reth_rpc_eth_types::{
    builder::botanix_config::Botanix, pending_block::pre_block_blockhashes_update, EthApiError, PendingBlock, PendingBlockEnv, PendingBlockEnvOrigin
};
use reth_transaction_pool::{BestTransactionsAttributes, TransactionPool};
use revm::{db::states::bundle_state::BundleRetention, DatabaseCommit, State};
use revm_primitives::Address;
use tokio::sync::Mutex;
use tracing::debug;

use crate::{EthApiTypes, FromEthApiError, FromEvmError};

use super::SpawnBlocking;

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

    fn botanix_provider(
        &self,
    ) -> &Botanix;

    fn get_gateway_address(
        &self,
        eth_address: Address,
    ) -> impl Future<Output = Result<Option<(bitcoin::Address, secp256k1::PublicKey)>, Self::Error>> + Send {
        async move {
            let pegin_info = self.botanix_provider().get_gateway_address(eth_address).await?;
            Ok(Some(pegin_info))
        }
    }

    fn get_merkle_proof(
        &self,
        txid: String,
        block_hash: String,
    ) -> impl Future<Output = Result<Vec<u8>, Self::Error>> + Send {
        async move {
            let pegin_info = self.botanix_provider().get_merkle_proof(txid, block_hash).await?;
            Ok(pegin_info)
        }
    }

    fn get_btc_fee_rate(&self) -> impl Future<Output = Result<U256, Self::Error>> + Send {
        async move {
            let fee_rate = self.botanix_provider().get_btc_fee_rate().await?;
            Ok(fee_rate)
        }
    }

}
