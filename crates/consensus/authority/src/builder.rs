use secp256k1::{All, Secp256k1};
use std::sync::Arc;
use tracing::error;
use url::Url;

use crate::{
    client::AuthorityClient, epoch_manager::EpochManager, task::BlockProductionTask,
    utils::get_authority_list, voting::AuthorityVote, AuthorityConsensus, Storage,
};
use client::BtcServerClient;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_primitives::ChainSpec;
use reth_provider::{BlockReaderIdExt, CanonStateNotificationSender, StateProviderFactory, CanonChainTracker};
use reth_transaction_pool::TransactionPool;
use reth_network::NetworkHandle;
use tokio::sync::{mpsc::UnboundedSender, RwLock};

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
    secp: Secp256k1<All>,
    sk: secp256k1::SecretKey,
    vote: Option<AuthorityVote>,
    epoch_manager: EpochManager,
    network_handle: NetworkHandle,
}

/// Errors that can occur when building an authority consensus.
#[derive(Debug)]    
pub enum AuthorityConsensusBuilderError {
    InvalidStorage,
    FailedToRecoverAuthorityList,
    FailedToFindSignerIndex,
    FailedToRetrieveEopchHeader,
}

// ===== impl AuthorityConsensusBuilder =====
impl<Client, Pool> AuthorityConsensusBuilder<Client, Pool>
where
    Client: BlockReaderIdExt + StateProviderFactory + CanonChainTracker,
    Pool: TransactionPool,
{
    /// Creates a new builder instance to configure all parts.
    pub fn try_new(
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
        network_handle: NetworkHandle,
    ) -> Result<Self, AuthorityConsensusBuilderError> {
        let mut latest_header = client
            .latest_header()
            .ok()
            .flatten()
            .unwrap_or_else(|| chain_spec.sealed_genesis_header());
        let mut headers = vec![latest_header.clone()];

        while !latest_header.is_poa_epoch() {
            let parent_hash = latest_header.parent_hash;
            
            if let Some(new_header) = client.header(&parent_hash).ok().flatten() {
                let old_latest_header = std::mem::replace(&mut latest_header, new_header.seal_slow());
                headers.push(old_latest_header);
            } else {
                return Err(AuthorityConsensusBuilderError::FailedToRetrieveEopchHeader);
            }
        }

        // Latest epoch header is the last header in the vector
        let authorities = get_authority_list(&latest_header).map_err(|e| {
            error!("Failed to retrieve authority list: {:?}", e);
            AuthorityConsensusBuilderError::FailedToRecoverAuthorityList
        })?;

        let signer_index = authorities.iter().position(|a| *a == sk.public_key(&secp));

        if signer_index.is_none() {
            return Err(AuthorityConsensusBuilderError::FailedToFindSignerIndex)
        }

        // Try to instantiate storage
        let storage = Storage::try_new(&mut headers, authorities, signer_index.expect("valid index"))
            .map_err(|e| {
                error!("Failed to instantiate storage: {:?}", e);
                AuthorityConsensusBuilderError::InvalidStorage
            })?;

        // Instantiate epoch manager
        let epoch_manager = EpochManager::naive_inverval(storage.clone());

        Ok(Self {
            storage,
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
            epoch_manager,
            network_handle,
        })
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
            epoch_manager,
            network_handle
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
            epoch_manager,
            network_handle,
        );

        (consensus, auth_client, task)
    }
}
