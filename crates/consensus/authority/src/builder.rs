use crate::{
    block_fetcher::BlockFetcherTask,
    epoch_manager::EpochManager,
    extended_client::BtcServerExtendedClient,
    frost_task::{FrostNotificationMessage, FrostTask},
    task::BlockProductionTask,
    voting::AuthorityVote,
    AuthorityConsensus, Storage,
};

use crate::sync::SyncController;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_btc_wallet::bitcoind::{BitcoindClient, BitcoindConfig};
use reth_consensus_common::utils::get_authority_list;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_network::{
    frost::manager::{FrostConfig, FrostHandle},
    message::NewBlockMessage,
    NetworkEvents, NetworkHandle,
};
use reth_node_api::{ConfigureEvmEnv, EngineTypes};
use reth_node_ethereum::EthEngineTypes;
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::ChainSpec;
use reth_provider::{
    BlockReaderIdExt, CanonChainTracker, CanonStateNotificationSender, StateProviderFactory,
};
use reth_tasks::TaskExecutor;
use secp256k1::{All, Secp256k1};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    RwLock,
};
use tracing::error;

/// Builder type for confirguring the setup
pub struct AuthorityConsensusBuilder<Client, EvmConfig, Engine: EngineTypes> {
    #[allow(dead_code)]
    client: Client,
    consensus: AuthorityConsensus,
    storage: Storage<Client>,
    to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
    canon_state_notification: CanonStateNotificationSender,
    btc_server: BtcServerExtendedClient,
    bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    bitcoin_block_tx_ids: Arc<RwLock<HashMap<u64, Vec<bitcoin::Txid>>>>,
    bitcoind_config: BitcoindConfig,
    secp: Secp256k1<All>,
    sk: secp256k1::SecretKey,
    #[allow(dead_code)]
    vote: Option<AuthorityVote>,
    epoch_manager: EpochManager<Client>,
    network_handle: NetworkHandle,
    frost_handle: Option<FrostHandle>,
    block_import_rx: UnboundedReceiver<NewBlockMessage>,
    task_executor: TaskExecutor,
    /// The type that defines how to configure the EVM.
    evm_config: EvmConfig,
    frost_config: FrostConfig,
    payload_builder: PayloadBuilderHandle<EthEngineTypes>,
    btc_network: bitcoin::Network,
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
impl<Client, EvmConfig, Engine> AuthorityConsensusBuilder<Client, EvmConfig, Engine>
where
    Engine: EngineTypes + 'static,
    EvmConfig:
        ConfigureEvmEnv + Clone + Unpin + Send + Sync + 'static + reth_node_api::ConfigureEvm,
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
{
    /// Creates a new builder instance to configure all parts.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        chain_spec: Arc<ChainSpec>,
        client: Client,
        to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
        canon_state_notification: CanonStateNotificationSender,
        btc_server: BtcServerExtendedClient,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        bitcoin_block_tx_ids: Arc<RwLock<HashMap<u64, Vec<bitcoin::Txid>>>>,
        bitcoind_config: BitcoindConfig,
        secp: Secp256k1<All>,
        // TODO (armins) This should be Arc protected
        sk: secp256k1::SecretKey,
        vote: Option<AuthorityVote>,
        network_handle: NetworkHandle,
        frost_handle: Option<FrostHandle>,
        block_import_rx: UnboundedReceiver<NewBlockMessage>,
        task_executor: TaskExecutor,
        evm_config: EvmConfig,
        frost_config: FrostConfig,
        payload_builder: PayloadBuilderHandle<EthEngineTypes>,
        btc_network: bitcoin::Network,
    ) -> Result<Self, AuthorityConsensusBuilderError> {
        let mut latest_header = client
            .latest_header()
            .ok()
            .flatten()
            .unwrap_or_else(|| chain_spec.sealed_genesis_header());
        let mut headers = vec![latest_header.clone()];

        while !latest_header.header().is_poa_epoch() {
            let parent_hash = latest_header.parent_hash;

            if let Some(new_header) = client.header(&parent_hash).ok().flatten() {
                let old_latest_header =
                    std::mem::replace(&mut latest_header, new_header.seal_slow());
                headers.push(old_latest_header);
            } else {
                return Err(AuthorityConsensusBuilderError::FailedToRetrieveEopchHeader);
            }
        }

        // Latest epoch header is the last header in the vector
        // This header should include the authority list which is validated by consensus
        let authorities = get_authority_list(&latest_header)
            .map_err(|e| {
                error!("Failed to retrieve authority list: {:?}", e);
                AuthorityConsensusBuilderError::FailedToRecoverAuthorityList
            })?
            .expect("authority signer list in epoch block");

        let signer_index = authorities.iter().position(|a| *a == sk.public_key(&secp));

        if signer_index.is_none() {
            return Err(AuthorityConsensusBuilderError::FailedToFindSignerIndex);
        }

        let pk = sk.public_key(&secp);

        // Try to instantiate storage
        let storage = Storage::try_new(
            client.clone(),
            &mut headers,
            authorities,
            signer_index.expect("valid index"),
            pk,
            btc_network,
        )
        .map_err(|e| {
            error!("Failed to instantiate storage: {:?}", e);
            AuthorityConsensusBuilderError::InvalidStorage
        })?;

        // Instantiate epoch manager
        let epoch_manager = EpochManager::<Client>::new(storage.clone());

        Ok(Self {
            storage,
            client,
            consensus: AuthorityConsensus::new(chain_spec),
            to_engine,
            canon_state_notification,
            btc_server,
            bitcoin_block_header,
            bitcoin_block_tx_ids,
            bitcoind_config,
            secp,
            sk,
            vote,
            epoch_manager,
            network_handle,
            frost_handle,
            block_import_rx,
            task_executor,
            evm_config,
            frost_config,
            payload_builder,
            btc_network,
        })
    }

    #[track_caller]
    /// Builds and returns the necessary components for the authority consensus, including the
    /// consensus itself, the client used to interact with the consensus, and the block
    /// production task.
    pub fn build(
        self,
    ) -> (
        AuthorityConsensus,
        BlockProductionTask<Client, EvmConfig, Engine>,
        BlockFetcherTask<Client, EvmConfig, Engine>,
        FrostTask<Client>,
        SyncController<Engine>,
    ) {
        let Self {
            btc_server,
            client: _,
            consensus,
            storage,
            to_engine,
            canon_state_notification,
            bitcoin_block_header,
            bitcoin_block_tx_ids,
            bitcoind_config,
            secp,
            sk,
            vote: _,
            epoch_manager,
            network_handle,
            frost_handle,
            block_import_rx,
            task_executor,
            evm_config,
            frost_config,
            payload_builder,
            btc_network,
        } = self;

        let sync_task = SyncController::new(
            network_handle.clone().event_listener(),
            *network_handle.peer_id(),
            to_engine.clone(),
        );

        let bitcoind_client =
            BitcoindClient::new(bitcoind_config.clone()).expect("Invalid Bitcoind client");
        let block_fetcher_task = crate::block_fetcher::BlockFetcherTask::new(
            Arc::clone(&consensus.chain_spec),
            block_import_rx,
            to_engine.clone(),
            canon_state_notification.clone(),
            btc_server.clone(),
            bitcoind_client,
            storage.clone(),
            bitcoin_block_header.clone(),
            evm_config.clone(),
            btc_network,
        );

        let (frost_task_notifications1_tx, frost_task_notifications1_rx) =
            tokio::sync::mpsc::unbounded_channel::<FrostNotificationMessage>();
        let (frost_task_notifications2_tx, frost_task_notifications2_rx) =
            tokio::sync::mpsc::unbounded_channel::<FrostNotificationMessage>();

        // TODO FIX the unwrap
        let frost_task = FrostTask::new(
            btc_server.clone(),
            network_handle.clone(),
            frost_handle.expect("Requires frost handle"),
            epoch_manager.clone(),
            frost_config,
            storage.clone(),
            frost_task_notifications1_rx,
            frost_task_notifications2_tx,
        );

        let bitcoind_client =
            BitcoindClient::new(bitcoind_config).expect("Invalid Bitcoind client");
        let block_production_task = BlockProductionTask::new(
            Arc::clone(&consensus.chain_spec),
            to_engine,
            canon_state_notification,
            storage,
            btc_server,
            bitcoin_block_header,
            bitcoin_block_tx_ids,
            bitcoind_client,
            secp,
            sk,
            epoch_manager,
            network_handle,
            frost_task.frost_handle.clone(),
            task_executor,
            evm_config.clone(),
            payload_builder,
            frost_task_notifications2_rx,
            frost_task_notifications1_tx,
            btc_network,
        );

        (consensus, block_production_task, block_fetcher_task, frost_task, sync_task)
    }
}
