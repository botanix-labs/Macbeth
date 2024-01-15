use crate::{epoch_manager::EpochManager, AuthorityConsensus, Storage};
use reth_beacon_consensus::{BeaconEngineMessage, ForkchoiceStatus};
use reth_botanix_lib::mint_validation::{
    parse_pegin_reth_log_topic, parse_pegout_reth_log_topic, GenesisContractEvents,
};
use reth_btc_wallet::block_source::{BlockSource, MempoolSpace};
use reth_consensus_common::{
    utils,
    utils::{validate_poa_block_beneficiary, validate_poa_extra_data_header},
    validation,
};
use reth_eth_wire::NewBlock;
use reth_interfaces::consensus::ForkchoiceState;
use reth_network::{message::NewBlockMessage, NetworkHandle};
use reth_primitives::{
    hex, Block, BlockBody, ChainSpec, IntoRecoveredTransaction, Log, SealedBlockWithSenders,
    TransactionSigned,
};
use reth_provider::{
    BundleStateWithReceipts, CanonChainTracker, CanonStateNotificationSender, Chain,
    StateProviderFactory,
};
use reth_revm::{database::StateProviderDatabase, processor::EVMProcessor, State};
use reth_rpc_types::engine::PayloadStatusEnum;
use reth_stages::PipelineEvent;
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};
use ruint::Uint;
use secp256k1::{All, Secp256k1};
use std::{collections::VecDeque, sync::Arc, task::Poll};

use tokio::sync::{
    mpsc::{error::TryRecvError, UnboundedReceiver, UnboundedSender},
    oneshot, RwLock,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, error, info, warn};
use url::Url;

use client::{BtcServerClient, MakeTxRequest, NotifyPeginRequest};

/// Repersents an error while processing a botanix log
#[derive(Debug, thiserror::Error)]
enum ProcessBotanixLogError {
    /// Failed to notify btc server about pegin
    #[error("Failed to notify btc server about pegin")]
    FailedToNotifyPegin(tonic::Status),
    #[error("Failed to broadcast pegout tx")]
    FailedToBroadcastPegout,
    #[error("Failed to make pegout tx")]
    FailedToMakePegoutTx(tonic::Status),
}

pub struct BlockProductionTask<Client, Pool: TransactionPool> {
    /// The configured chain spec
    chain_spec: Arc<ChainSpec>,
    /// The client used to interact with the state
    /// Note this is a database client
    client: Client,
    /// The active epoch
    epoch_manager: EpochManager,
    /// Shared storage to insert new blocks
    storage: Storage,
    /// Pool where transactions are stored
    pool: Pool,
    /// backlog of sets of transactions ready to be mined
    queued: VecDeque<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>,
    /// TODO: ideally this would just be a sender of hashes
    to_engine: UnboundedSender<BeaconEngineMessage>,
    /// Used to notify consumers of new blocks
    canon_state_notification: CanonStateNotificationSender,
    /// The pipeline events to listen on
    pipe_line_events: Option<UnboundedReceiverStream<PipelineEvent>>,
    /// BTC Server client
    btc_server: BtcServerClient<tonic::transport::Channel>,
    /// Recent bitcoin block headers
    bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    /// Bitcoin block source
    bitcoin_block_source: MempoolSpace,
    /// Instance of secp
    secp: Secp256k1<All>,
    /// Key of authority
    sk: secp256k1::SecretKey,
    /// Network Handler
    network_handle: NetworkHandle,
    /// Events from block import
    block_import_rx: UnboundedReceiver<NewBlockMessage>,
}

impl<Client, Pool: TransactionPool> BlockProductionTask<Client, Pool>
where
    Client: StateProviderFactory + CanonChainTracker + Clone + 'static,
    Pool: TransactionPool,
{
    /// Creates a new instance of the task
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        to_engine: UnboundedSender<BeaconEngineMessage>,
        canon_state_notification: CanonStateNotificationSender,
        storage: Storage,
        client: Client,
        pool: Pool,
        btc_server: BtcServerClient<tonic::transport::Channel>,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        bitcoin_block_source_address: Url,
        secp: Secp256k1<All>,
        sk: secp256k1::SecretKey,
        epoch_manager: EpochManager,
        network_handle: NetworkHandle,
        block_import_rx: UnboundedReceiver<NewBlockMessage>,
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
        }
    }

    pub async fn start_task(&mut self) -> () {
        // This drives block production
        loop {
            let new_block = match self.block_import_rx.try_recv() {
                Ok(b) => b,
                Err(error) => match error {
                    TryRecvError::Empty => {
                        debug!(target: "consensus::authority", "No new blocks from peers");
                        continue
                    }
                    TryRecvError::Disconnected => {
                        error!(target: "consensus::authority", "Block import channel disconnected");
                        return ()
                    }
                },
            };

            // Recieved a new block from a peer. Block import has ran consensus validation
            // against this block Update internal cache and notify the
            // engine
            loop {
                let block = new_block.block.block.clone();
                info!(target: "consensus::authority", ?block, "Recieved new block from peer");

                // extract signer pub key
                let signer = utils::recovery_authority(&block.header).expect("valid signer");

                let authorities = self.epoch_manager.storage.inner.read().await.authorities.clone();
                let signer_index =
                    authorities.iter().position(|pk| *pk == signer).expect("valid signer");
                match AuthorityConsensus::validate_inturn(
                    block.header.timestamp,
                    authorities.len() as u64,
                    signer_index as u64,
                ) {
                    Ok(_) => {}
                    Err(err) => {
                        error!(target: "consensus::authority", ?err, "Block import failed in turn check");
                        continue
                    }
                }

                // validate beneficiary is within the authorities list
                match validate_poa_block_beneficiary(&block.header, &authorities) {
                    Ok(_) => {}
                    Err(err) => {
                        error!(target: "consensus::authority", ?err, "Block beneficiary not found in authorities list");
                        continue
                    }
                }

                // send the new update to the engine, this will trigger the engine
                // to download and execute the block we just inserted
                let (tx, rx) = oneshot::channel();
                let sealed_block = block.clone().seal_slow();
                let _ = self.to_engine.send(BeaconEngineMessage::NewPayload {
                    payload: sealed_block.clone().into(),
                    cancun_fields: None,
                    tx,
                });

                let payload_status = match rx.await.unwrap() {
                    Ok(s) => s,
                    Err(status_err) => {
                        error!(target: "consensus::authority", ?status_err, "Authority fork new payload failed");
                        return ()
                    }
                };
                match payload_status.status {
                    PayloadStatusEnum::Accepted | PayloadStatusEnum::Valid => {}
                    PayloadStatusEnum::Invalid { validation_error } => {
                        error!(target: "consensus::authority", ?validation_error,
                            "Authority fork new payload returned invalid response"
                        );
                        break
                    }
                    PayloadStatusEnum::Syncing => {
                        debug!(target: "consensus::authority", ?payload_status,
                            "Authority fork new payload returned SYNCING, waiting for VALID"
                        );
                        // wait for the next fork choice update
                        continue
                    }
                };
                // remove the tx which are now confirmed
                info!("Removing txs from the pool upon recevied block");
                let tx_hashes =
                    block.body.iter().map(|tx| tx.hash().to_owned()).collect::<Vec<_>>();
                self.pool.remove_transactions(tx_hashes);

                let senders =
                    TransactionSigned::recover_signers(&block.body, block.body.len()).unwrap();

                let db = State::builder()
                    .with_database_boxed(Box::new(StateProviderDatabase::new(
                        self.client.latest().unwrap(),
                    )))
                    .with_bundle_update()
                    .build();
                let mut executor = EVMProcessor::new_with_state(self.chain_spec.clone(), db);
                let mut storage = self.storage.write().await;
                let recent_bitcoin_block_header = self.bitcoin_block_header.read().await.clone();

                match storage.execute(
                    &block,
                    &mut executor,
                    senders.clone(),
                    recent_bitcoin_block_header,
                ) {
                    Ok((bundle_state, _gas_used)) => {
                        drop(storage);
                        let sealed_block_with_senders =
                            SealedBlockWithSenders::new(sealed_block, senders)
                                .expect("senders are valid");
                        self.persist_new_block(sealed_block_with_senders.clone(), bundle_state)
                            .await;
                    }
                    Err(err) => {
                        error!(target: "consensus::authority", ?err, "Failed to exectute block recieved by peer");
                    }
                }
                break
            }

            let is_inturn = match self.epoch_manager.poll(&self.pool).await {
                (Poll::Pending, is_inturn) => is_inturn,
                (Poll::Ready(transactions), is_inturn) => {
                    info!(
                        "Adding to the list of transctions, {:?}, {:?}",
                        transactions, self.queued
                    );
                    self.queued.push_back(transactions.clone());
                    let mining_pool = self.pool.clone();
                    mining_pool.remove_transactions(
                        transactions.iter().map(|tx| tx.hash().to_owned()).collect(),
                    );
                    is_inturn
                }
            };

            // If insert task is not none executinon of async task is on going
            if self.queued.is_empty() || !is_inturn {
                info!("Txs list is empty, skipping");
                // nothing to insert
                std::thread::sleep(std::time::Duration::from_millis(1000));
                continue
            }

            // ready to queue in new insert task
            let transactions = self.queued.pop_front().expect("not empty");
            let txs_cloned = transactions.clone();

            let events = self.pipe_line_events.take();

            let client = self.client.clone();

            // Create the mining future that creates a block, notifies the engine that drives
            // the pipeline

            let (transactions, senders): (Vec<_>, Vec<_>) = transactions
                .into_iter()
                .map(|tx| {
                    let recovered = tx.to_recovered_transaction();
                    let signer = recovered.signer();
                    (recovered.into_signed(), signer)
                })
                .unzip();
            let mut storage = self.storage.write().await;
            let recent_bitcoin_block_header = self.bitcoin_block_header.read().await.clone();
            let authority_signers = storage.authorities.clone();
            // execute the new block
            let (new_header, bundle_state) = match storage.build_and_execute(
                transactions.clone(),
                &client,
                self.chain_spec.clone(),
                recent_bitcoin_block_header,
                // TODO(armins) read vote in as param
                &None,
                &self.sk,
                &self.secp,
                &authority_signers,
            ) {
                Ok(ret) => ret,
                Err(err) => {
                    error!(target: "consensus::authority", ?err, "failed to execute block");
                    drop(storage);
                    self.queued.push_front(txs_cloned);
                    continue
                }
            };
            drop(storage);
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
                            match self.process_botanix_log(log).await {
                                Ok(_) => {}
                                Err(err) => {
                                    error!(target: "consensus::authority", ?err, "Failed to process botanix log");
                                }
                            }
                        }
                    }

                    info!("Reciept {:?}", reciept);
                }
            }

            // seal the block
            let block = Block {
                header: new_header.clone().unseal(),
                body: transactions,
                ommers: vec![],
                withdrawals: None,
            };
            let sealed_block = block.clone().seal_slow();
            let sealed_block_with_senders =
                SealedBlockWithSenders::new(sealed_block, senders).expect("senders are valid");
            self.persist_new_block(sealed_block_with_senders.clone(), bundle_state).await;
            // Notify peers
            let new_block = NewBlock { block, td: Uint::ZERO };
            let block_hash = sealed_block_with_senders.hash();
            self.network_handle.announce_block(new_block, block_hash);

            self.pipe_line_events = events;
        }
    }

    async fn process_botanix_log(&mut self, log: &Log) -> Result<(), ProcessBotanixLogError> {
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

    async fn persist_new_block(
        &mut self,
        sealed_block: SealedBlockWithSenders,
        bundled_state: BundleStateWithReceipts,
    ) -> () {
        let new_header = sealed_block.header.clone();
        // perform PoA validation
        let authority_signers = self.storage.read().await.authorities.clone();
        // TODO (armins) remove this unwrap
        validate_poa_extra_data_header(&new_header, &authority_signers).unwrap();

        let state = ForkchoiceState {
            head_block_hash: new_header.hash,
            finalized_block_hash: new_header.hash,
            safe_block_hash: new_header.hash,
        };

        loop {
            // send the new update to the engine, this will trigger
            // the engine
            // to download and execute the block we just inserted
            let (tx, rx) = oneshot::channel();
            let _ = self.to_engine.send(BeaconEngineMessage::ForkchoiceUpdated {
                state,
                payload_attrs: None,
                tx,
            });
            debug!(target: "consensus::authority", ?state, "Sent fork choice update");

            match rx.await.unwrap() {
                Ok(fcu_response) => {
                    match fcu_response.forkchoice_status() {
                        ForkchoiceStatus::Valid => break,
                        ForkchoiceStatus::Invalid => {
                            error!(target: "consensus::authority", ?fcu_response, "Forkchoice update returned invalid response");
                            return ()
                        }
                        ForkchoiceStatus::Syncing => {
                            debug!(target: "consensus::authority", ?fcu_response, "Forkchoice update returned SYNCING, waiting for VALID");
                            // wait for the next fork choice update
                            continue
                        }
                    }
                }
                Err(err) => {
                    error!(target: "consensus::authority", ?err, "Authority fork choice update failed");
                    return ()
                }
            }
        }

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
        self.storage
            .write()
            .await
            .insert_new_block(sealed_block.header.header.clone(), body.clone());

        let mining_pool = self.pool.clone();
        // Lastly remove confirmed txs from the mempool
        mining_pool
            .remove_transactions(body.transactions.iter().map(|tx| tx.hash().to_owned()).collect());
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
