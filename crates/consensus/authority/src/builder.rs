use secp256k1::{All, Secp256k1};
use std::sync::Arc;
use url::Url;

use crate::{
    client::AuthorityClient, task::BlockProductionTask, voting::AuthorityVote, AuthorityConsensus,
    Storage,
};
use client::BtcServerClient;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_primitives::ChainSpec;
use reth_provider::{BlockReaderIdExt, CanonStateNotificationSender};
use reth_transaction_pool::TransactionPool;
use tokio::sync::{mpsc::UnboundedSender, RwLock};

/// Builder type for confirguring the setup
pub struct AuthorityConsensusBuilder<Client, Pool> {
    client: Client,
    consensus: AuthorityConsensus,
    pool: Pool,
    mode: MiningMode,
    storage: Storage,
    to_engine: UnboundedSender<BeaconEngineMessage>,
    canon_state_notification: CanonStateNotificationSender,
    btc_server: BtcServerClient<tonic::transport::Channel>,
    bitcoin_block_header: Arc<RwLock<Option<bitcoin::block::Header>>>,
    bitcoin_block_source_address: Url,
    secp: Secp256k1<All>,
    sk: secp256k1::SecretKey,
    vote: Option<AuthorityVote>,
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
        secp: Secp256k1<All>,
        // TODO (armins) This should be Arc protected   
        sk: secp256k1::SecretKey,
        vote: Option<AuthorityVote>,
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
            bitcoin_block_source_address,
            secp,
            sk,
            vote,
        }
    }

    #[track_caller]
    pub fn build(self) -> (AuthorityConsensus, AuthorityClient, BlockProductionTask<Client, Pool>) {
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
            secp,
            sk,
            vote,
        } = self;
        let auth_client = AuthorityClient::new(storage.clone());

        let task = BlockProductionTask::new(
            Arc::clone(&consensus.chain_spec),
            to_engine,
            canon_state_notification,
            storage,
            client,
            pool,
            btc_server,
            bitcoin_block_header,
            bitcoin_block_source_address,
            secp,
            sk,
        );

        (consensus, auth_client, task)
    }
}
