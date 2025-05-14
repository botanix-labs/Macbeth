use reth_chainspec::ChainSpec;
use reth_evm_ethereum::EthEvmConfig;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

/// In memory storage
/// All this struct does is provide a rwlock wrapper around the storage inner
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct Storage<EF, BF, DB> {
    pub(crate) client: DB,
    /// The authority list in the genesis block
    pub(crate) genesis_authorities: Vec<secp256k1::PublicKey>,
    /// keep track of my place among the signer
    /// This will change as new signers are removed
    pub(crate) signer_index: usize,
    /// Authority Signer public key
    pub(crate) authority: secp256k1::PublicKey,
    /// Bitcoin network
    pub(crate) btc_network: bitcoin::Network,
    /// Authority socket addresses pulled from federation config
    pub(crate) authority_socket_addresses: Vec<SocketAddr>,
    /// Evm config
    pub(crate) evm_config: EthEvmConfig,
    /// Bitcoind Factory
    pub(crate) bitcoind_factory: BF,
    /// Chain spec
    pub(crate) chain_spec: Arc<ChainSpec>,
    /// Executor Factory
    pub(crate) executor_factory: EF,
    // The inner storage, everything here is rw locked
    pub(crate) inner: Arc<RwLock<StorageInner>>,
}

impl<EF, BF, DB: Clone> Storage<EF, BF, DB> {
    /// Create a new instance of the storage
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        genesis_authorities: Vec<secp256k1::PublicKey>,
        signer_index: usize,
        authority: secp256k1::PublicKey,
        btc_network: bitcoin::Network,
        aggregate_public_key: Option<secp256k1::PublicKey>,
        authority_socket_addresses: Vec<SocketAddr>,
        evm_config: EthEvmConfig,
        chain_spec: Arc<ChainSpec>,
        bitcoind_factory: BF,
        executor_factory: EF,
        client: DB,
    ) -> Self {
        let storage_inner = StorageInner { aggregate_public_key, is_block_syncing: false };

        Self {
            client,
            genesis_authorities,
            signer_index,
            authority,
            btc_network,
            authority_socket_addresses,
            evm_config,
            chain_spec,
            bitcoind_factory,
            executor_factory,
            inner: Arc::new(RwLock::new(storage_inner)),
        }
    }

    /// Returns the write lock of the storage
    pub(crate) async fn write(&self) -> RwLockWriteGuard<'_, StorageInner> {
        self.inner.write().await
    }

    #[allow(dead_code)]
    /// Returns the read lock of the storage
    pub(crate) async fn read(&self) -> RwLockReadGuard<'_, StorageInner> {
        self.inner.read().await
    }
}

#[derive(Debug)]
/// In-memory storage for the chain the authority seal engine is building.
/// data shared amongst the different tasks should be stored here and protected by a rwlock
pub(crate) struct StorageInner {
    /// The aggregate public key of the FROST threshold signature scheme
    /// Should get populated after DKG
    pub(crate) aggregate_public_key: Option<secp256k1::PublicKey>,
    /// Suggests if we are currently syncing blocks
    pub(crate) is_block_syncing: bool,
}
