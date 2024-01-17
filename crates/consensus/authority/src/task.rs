use crate::{
    engine_util::{self, SendForkChoiceUpdateError},
    epoch_manager::EpochManager,
    Storage,
};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_botanix_lib::mint_validation::{
    parse_pegin_reth_log_topic, parse_pegout_reth_log_topic, GenesisContractEvents,
};
use reth_btc_wallet::block_source::{BlockSource, MempoolSpace};
use reth_consensus_common::utils::validate_poa_extra_data_header;


use crate::sync::SyncController;
use reth_interfaces::consensus::ConsensusError;
use reth_network::{message::NewBlockMessage, NetworkEvents, NetworkHandle};
use reth_primitives::{hex, BlockBody, ChainSpec, Log, SealedBlockWithSenders};
use reth_provider::{
    BlockReaderIdExt, BundleStateWithReceipts, CanonChainTracker, CanonStateNotificationSender,
    Chain, StateProviderFactory,
};
use reth_stages::PipelineEvent;
use reth_tasks::TaskExecutor;
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};

use secp256k1::{All, Secp256k1};
use std::{collections::VecDeque, sync::Arc};

use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    RwLock,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, error, info};
use url::Url;

use client::{BtcServerClient, MakeTxRequest, NotifyPeginRequest};

/// Repersents an error while processing a botanix log
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessBotanixLogError {
    /// Failed to notify btc server about pegin
    #[error("Failed to notify btc server about pegin")]
    FailedToNotifyPegin(tonic::Status),
    #[error("Failed to broadcast pegout tx")]
    FailedToBroadcastPegout,
    #[error("Failed to make pegout tx")]
    FailedToMakePegoutTx(tonic::Status),
}

/// Persist new block Errors
#[derive(Debug, thiserror::Error)]
pub(crate) enum PersistNewBlockError {
    #[error("Failed to validate PoA header")]
    FailedToValidatePoaHeader(ConsensusError),
    #[error("Failed to communicate with engine API")]
    FailedToCommunicateWithEngine(SendForkChoiceUpdateError),
}

pub struct BlockProductionTask<Client, Pool: TransactionPool> {
    /// The configured chain spec
    pub(crate) chain_spec: Arc<ChainSpec>,
    /// The client used to interact with the state
    /// Note this is a database client
    pub(crate) client: Client,
    /// The active epoch
    pub(crate) epoch_manager: EpochManager<Client>,
    /// Shared storage to insert new blocks
    pub(crate) storage: Storage<Client>,
    /// Pool where transactions are stored
    pub(crate) pool: Pool,
    /// backlog of sets of transactions ready to be mined
    pub(crate) queued:
        VecDeque<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>,
    /// TODO: ideally this would just be a sender of hashes
    pub(crate) to_engine: UnboundedSender<BeaconEngineMessage>,
    /// Used to notify consumers of new blocks
    pub(crate) canon_state_notification: CanonStateNotificationSender,
    /// The pipeline events to listen on
    pub(crate) pipe_line_events: Option<UnboundedReceiverStream<PipelineEvent>>,
    /// BTC Server client
    pub(crate) btc_server: BtcServerClient<tonic::transport::Channel>,
    /// Recent bitcoin block headers
    pub(crate) bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    /// Bitcoin block source
    pub(crate) bitcoin_block_source: MempoolSpace,
    /// Instance of secp
    pub(crate) secp: Secp256k1<All>,
    /// Key of authority
    pub(crate) sk: secp256k1::SecretKey,
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    /// Events from block import
    pub(crate) block_import_rx: UnboundedReceiver<NewBlockMessage>,
    /// Task executor
    #[allow(dead_code)]
    task_executor: TaskExecutor,
}

impl<Client, Pool: TransactionPool> BlockProductionTask<Client, Pool>
where
    Client: BlockReaderIdExt + StateProviderFactory + CanonChainTracker + Clone + 'static,
    Pool: TransactionPool,
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        to_engine: UnboundedSender<BeaconEngineMessage>,
        canon_state_notification: CanonStateNotificationSender,
        storage: Storage<Client>,
        client: Client,
        pool: Pool,
        btc_server: BtcServerClient<tonic::transport::Channel>,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        bitcoin_block_source_address: Url,
        secp: Secp256k1<All>,
        sk: secp256k1::SecretKey,
        epoch_manager: EpochManager<Client>,
        network_handle: NetworkHandle,
        block_import_rx: UnboundedReceiver<NewBlockMessage>,
        task_executor: TaskExecutor,
    ) -> Self {
        Self {
            chain_spec,
            client,
            storage,
            pool,
            to_engine,
            canon_state_notification,
            queued: Default::default(),
            pipe_line_events: None,
            btc_server,
            bitcoin_block_header,
            bitcoin_block_source: MempoolSpace::new(bitcoin_block_source_address.to_string()),
            secp,
            sk,
            epoch_manager,
            network_handle,
            block_import_rx,
            task_executor,
        }
    }

    pub async fn start_task(&mut self) -> () {
        let mut sync_controller = SyncController::new(
            self.network_handle.event_listener(),
            self.network_handle.peer_id().clone(),
            self.to_engine.clone(),
        );

        self.task_executor.spawn_critical(
            "Sync Controller",
            Box::pin(async move {
                sync_controller.try_sync_peer_tip().await;
            }),
        );

        loop {
            self.try_fetch_block().await;
            self.try_build_block().await;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    /// Processes the reciepts of a block
    pub(crate) async fn process_reciepts(
        &mut self,
        bundle_state: &BundleStateWithReceipts,
        should_broadcast_pegout: bool,
    ) -> Result<(), ProcessBotanixLogError> {
        let reciepts_bundle = bundle_state.receipts().iter();
        for (index, reciepts) in reciepts_bundle.enumerate() {
            for reciept in reciepts {
                if index == 0 && reciept.is_none() {
                    // Prunning block, skip
                    break
                }
                if let Some(reciept) = reciept {
                    if !reciept.success {
                        continue
                    }
                    for log in &reciept.logs {
                        self.process_botanix_log(log, should_broadcast_pegout).await?;
                    }
                }
                info!(target: "consensus::authority", "Reciept {:?}", reciept);
            }
        }
        Ok(())
    }

    pub(crate) async fn process_botanix_log(
        &mut self,
        log: &Log,
        should_broadcast_pegout: bool,
    ) -> Result<(), ProcessBotanixLogError> {
        for topic in &log.topics {
            match GenesisContractEvents::try_from(topic.clone()) {
                Ok(GenesisContractEvents::MintingEvent) => {
                    info!(target: "consensus::authority", "Parsing and sending minting event to btc_server");
                    let pegin_data = parse_pegin_reth_log_topic(&log)
                        .expect("passed evm check should pass this parse attempt");
                    for pegin in &pegin_data.meta {
                        let request = NotifyPeginRequest {
                            utxo_txid: pegin.outpoint.txid.to_string(),
                            utxo_vout: pegin.outpoint.vout,
                            eth_address: hex::encode(pegin.address.to_vec()),
                            output: bitcoin::consensus::serialize(
                                pegin
                                    .tx
                                    .output
                                    .get(pegin.outpoint.vout as usize)
                                    .expect("valid vout"),
                            ),
                        };
                        self.btc_server
                            .notify_pegin(request)
                            .await
                            .map_err(|e| ProcessBotanixLogError::FailedToNotifyPegin(e))?;
                        info!(target: "consensus::authority", "notifying btc server about pegin utxo");
                    }
                }
                Ok(GenesisContractEvents::BurnEvent) => {
                    if !should_broadcast_pegout {
                        continue
                    }
                    // TODO (armins): obv
                    let fee_rate = 30u32;
                    info!(target: "consensus::authority", "Parsing and sending withdrawal event to btc_server");
                    let pegout = parse_pegout_reth_log_topic(&log).expect("valid pegout request");
                    let request = MakeTxRequest {
                        address: pegout.destination.to_string(),
                        value: pegout.amount.to_sat(),
                        fee_rate,
                    };

                    let response = self
                        .btc_server
                        .make_tx(request)
                        .await
                        .map_err(|e| ProcessBotanixLogError::FailedToMakePegoutTx(e))?;

                    let raw_tx = response.into_inner().tx;
                    info!(target: "consensus::authority", "broadcasting withdrawal tx");

                    self.bitcoin_block_source
                        .broadcast_tx(&hex::encode(raw_tx))
                        .await
                        .map_err(|_| ProcessBotanixLogError::FailedToBroadcastPegout)?;
                }
                Err(e) => {
                    debug!(target: "consensus::authority", ?e, "Non-genesis contract event");
                    continue
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn persist_new_block(
        &mut self,
        sealed_block: SealedBlockWithSenders,
        bundled_state: BundleStateWithReceipts,
    ) -> Result<(), PersistNewBlockError> {
        let new_header = sealed_block.header.clone();
        // perform PoA validation
        let storage = self.storage.read().await;
        let authority_signers = storage.authorities.clone();
        drop(storage);
        validate_poa_extra_data_header(&new_header, &authority_signers)
            .map_err(|e| PersistNewBlockError::FailedToValidatePoaHeader(e))?;


        // Try to notify the engine of the new block
        engine_util::send_fork_choice_update_payload(
            new_header.clone().hash,
            self.to_engine.clone(),
        )
        .await
        .map_err(|e| PersistNewBlockError::FailedToCommunicateWithEngine(e))?;

        // update canon chain for rpc
        self.client.set_canonical_head(sealed_block.header.clone());
        self.client.set_safe(sealed_block.header.clone());
        self.client.set_finalized(sealed_block.header.clone());

        let chain = Arc::new(Chain::new(vec![sealed_block.clone()], bundled_state));

        info!(target: "consensus::authority", "sending block notification to block chain tree");
        // send block notification
        let _ = self
            .canon_state_notification
            .send(reth_provider::CanonStateNotification::Commit { new: chain });

        // Update internal consensus cache
        let body = BlockBody {
            transactions: sealed_block.body.clone(),
            ommers: vec![],
            withdrawals: None,
        };
        let mining_pool = self.pool.clone();
        // Lastly remove confirmed txs from the mempool
        mining_pool
            .remove_transactions(body.transactions.iter().map(|tx| tx.hash().to_owned()).collect());

        Ok(())
    }

    /// Sets the pipeline events to listen on.
    pub fn set_pipeline_events(&mut self, events: UnboundedReceiverStream<PipelineEvent>) {
        self.pipe_line_events = Some(events);
    }
}

impl<Client, Pool: TransactionPool> std::fmt::Debug for BlockProductionTask<Client, Pool> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockProductionTask").finish_non_exhaustive()
    }
}
