use std::sync::Arc;
use url::Url;

use crate::AuthorityConsensus;
use client::BtcServerClient;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_primitives::ChainSpec;
use reth_provider::{BlockReaderIdExt, PostState, StateProvider, CanonStateNotificationSender};
use reth_transaction_pool::TransactionPool;
use tokio::sync::{mpsc::UnboundedSender, RwLock, RwLockReadGuard, RwLockWriteGuard};
use crate::StorageInner;
use crate::Storage;

/// Builder type for confirguring the setup
pub struct AuthorityConsensusBuilder<Client, Pool> {
    client: Client,
    consensus: AuthorityConsensus,
    pool: Pool,
    storage: Storage,
    to_engine: UnboundedSender<BeaconEngineMessage>,
    canon_state_notification: CanonStateNotificationSender,
    btc_server: BtcServerClient<tonic::transport::Channel>,
    bitcoin_block_header: Arc<RwLock<Option<bitcoin::block::Header>>>,
    bitcoin_block_source_address: Url,
}

// ===== impl AuthorityConsensusBuilder =====
impl<Client, Pool> AuthorityConsensusBuilder<Client, Pool>
where
    Client: BlockReaderIdExt,
    Pool: TransactionPool,
{
    /// Creates a new builder instance to configure all parts.
    pub fn new(
        chain_spec: Arc<ChainSpec>,
        client: Client,
        pool: Pool,
        to_engine: UnboundedSender<BeaconEngineMessage>,
        canon_state_notification: CanonStateNotificationSender,
        btc_server: BtcServerClient<tonic::transport::Channel>,
        bitcoin_block_header: Arc<RwLock<Option<bitcoin::block::Header>>>,
        bitcoin_block_source_address: Url,
    ) -> Self {
        let latest_header = client
            .latest_header()
            .ok()
            .flatten()
            .unwrap_or_else(|| chain_spec.sealed_genesis_header());

        Self {
            storage: Storage::new(latest_header),
            client,
            consensus: AuthorityConsensus::new(chain_spec),
            pool,
            to_engine,
            canon_state_notification,
            btc_server,
            bitcoin_block_header,
            bitcoin_block_source_address
        }
    }

    #[track_caller]
    pub fn build(self) -> AuthorityConsensus {
        let Self {
            btc_server,
            client,
            consensus,
            pool,
            storage,
            to_engine,
            canon_state_notification,
            bitcoin_block_header,
            bitcoin_block_source_address,
        } = self;

        //TODO: instantiate a new mining task

        consensus
    }
}
