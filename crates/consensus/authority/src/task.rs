use crate::{epoch_manager::EpochManager, Storage};
use botanix_lib::mint_validation::{
    parse_pegin_reth_log_topic, parse_pegout_reth_log_topic, GenesisContractEvents,
};

use btc_wallet::block_source::{BlockSource, MempoolSpace};
use futures_util::future::BoxFuture;
use reth_beacon_consensus::{BeaconEngineMessage, ForkchoiceStatus};
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{hex, Block, ChainSpec, IntoRecoveredTransaction, SealedBlockWithSenders};
use reth_provider::{CanonChainTracker, CanonStateNotificationSender, Chain, StateProviderFactory};
use reth_revm::{
    database::{State, SubState},
    executor::Executor,
};
use reth_stages::PipelineEvent;
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};
use secp256k1::{All, Secp256k1};
use std::{collections::VecDeque, sync::Arc, task::Poll};
use tokio::sync::{mpsc::UnboundedSender, oneshot, RwLock};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, error, info, warn};
use url::Url;

use client::{BtcServerClient, MakeTxRequest, NotifyPeginRequest};

pub struct BlockProductionTask<Client, Pool: TransactionPool> {
    /// The configured chain spec
    chain_spec: Arc<ChainSpec>,
    /// The client used to interact with the state
    client: Client,
    /// The active epoch
    epoch_manager: EpochManager,
    /// Single active future that inserts a new block into `storage`
    insert_task: Option<BoxFuture<'static, Option<UnboundedReceiverStream<PipelineEvent>>>>,
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
    bitcoin_block_header: Arc<RwLock<Option<bitcoin::block::Header>>>,
    /// Bitcoin block source url
    bitcoin_block_source_address: Url,
    /// Instance of secp
    secp: Secp256k1<All>,
    /// Key of authority
    sk: secp256k1::SecretKey,
}

impl<Client, Pool: TransactionPool> BlockProductionTask<Client, Pool> 
where 
    Client: StateProviderFactory + CanonChainTracker ,
    Pool: TransactionPool
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
        bitcoin_block_header: Arc<RwLock<Option<bitcoin::block::Header>>>,
        bitcoin_block_source_address: Url,
        secp: Secp256k1<All>,
        sk: secp256k1::SecretKey,
        epoch_manager: EpochManager,
    ) -> Self {
        Self {
            chain_spec,
            client,
            insert_task: None,
            storage,
            pool,
            to_engine,
            canon_state_notification,
            queued: Default::default(),
            pipe_line_events: None,
            btc_server,
            bitcoin_block_header,
            bitcoin_block_source_address,
            secp,
            sk,
            epoch_manager,
        }
    }

    pub async fn start_task(&mut self) -> () {
        // this drives block production
        loop {
            if let Poll::Ready(transactions) = self.epoch_manager.poll(&self.pool).await {
                info!("Adding to the list of transctions, {:?}, {:?}", transactions, self.queued);
                // miner returned a set of transaction that we feed to
                // the producer
                self.queued.push_back(transactions.clone());
                let mining_pool = self.pool.clone();
                mining_pool.remove_transactions(
                    transactions.iter().map(|tx| tx.hash().to_owned()).collect(),
                );
            }

            // If insert task is not none executinon of async task is on going
            if self.queued.is_empty() {
                info!("Txs list is empty, skipping");
                // nothing to insert
                std::thread::sleep(std::time::Duration::from_millis(1000));
                continue;
            }

            // ready to queue in new insert task
            let transactions = self.queued.pop_front().expect("not empty");

            let events = self.pipe_line_events.take();
            let block_source =
                MempoolSpace::new(self.bitcoin_block_source_address.clone().to_string());

            // Create the mining future that creates a block, notifies the engine that drives
            // the pipeline
            let recent_block_header = self.bitcoin_block_header.read().await.clone();
            let mut storage = self.storage.write().await;

            let (transactions, senders): (Vec<_>, Vec<_>) = transactions
                .into_iter()
                .map(|tx| {
                    let recovered = tx.to_recovered_transaction();
                    let signer = recovered.signer();
                    (recovered.into_signed(), signer)
                })
                .unzip();

            // execute the new block
            let substate = SubState::new(State::new(self.client.latest().unwrap()));
            let mut executor = Executor::new(Arc::clone(&self.chain_spec), substate);
            match storage.build_and_execute(
                transactions.clone(),
                &mut executor,
                &self.chain_spec,
                recent_block_header,
                // TODO(armins) read vote in as param
                &None,
                &self.sk,
                &self.secp,
            ) {
                Ok((new_header, post_state)) => {
                    let state = ForkchoiceState {
                        head_block_hash: new_header.hash,
                        finalized_block_hash: new_header.hash,
                        safe_block_hash: new_header.hash,
                    };
                    drop(storage);
                    for reciept in post_state.receipts(new_header.number) {
                        if !reciept.success {
                            continue
                        }
                        for log in &reciept.logs {
                            for topic in &log.topics {
                                match GenesisContractEvents::try_from(topic.clone()) {
                                    Ok(GenesisContractEvents::MintingEvent) => {
                                        info!("Parsing and sending minting event to btc_server");
                                        let pegin_data = parse_pegin_reth_log_topic(&log).expect(
                                            "passed evm check should pass this parse attempt",
                                        );

                                        let request = NotifyPeginRequest {
                                            utxo_txid: pegin_data.meta.outpoint.txid.to_string(),
                                            utxo_vout: pegin_data.meta.outpoint.vout,
                                            eth_address: hex::encode(
                                                pegin_data.meta.address.to_vec(),
                                            ),
                                            output: bitcoin::consensus::serialize(
                                                &pegin_data
                                                    .meta
                                                    .tx
                                                    .output
                                                    .get(pegin_data.meta.outpoint.vout as usize)
                                                    .unwrap(),
                                            ),
                                            nonce: pegin_data.nonce,
                                        };
                                        self.btc_server.notify_pegin(request).await.unwrap();
                                        info!("notifying btc server about pegin utxo");
                                    }
                                    Ok(GenesisContractEvents::BurnEvent) => {
                                        // TODO (armins): obv
                                        let fee_rate = 30u32;
                                        info!("Parsing and sending withdrawal event to btc_server");
                                        let pegout = parse_pegout_reth_log_topic(&log)
                                            .expect("valid pegout request");
                                        let request = MakeTxRequest {
                                            address: pegout.destination.to_string(),
                                            value: pegout.amount.to_sat(),
                                            fee_rate,
                                        };

                                        match self.btc_server.make_tx(request).await {
                                            Ok(response) => {
                                                let raw_tx = response.into_inner().tx;
                                                info!(
                                                    "Pegout tx from btc signer service {:?}",
                                                    raw_tx.clone()
                                                );

                                                match block_source
                                                    .broadcast_tx(&hex::encode(raw_tx))
                                                    .await
                                                {
                                                    Ok(tx_response) => {
                                                        info!("Broadcasted withdrawal tx with txid: {}", tx_response);
                                                    }
                                                    Err(err) => {
                                                        error!("Warning: Failed to broadcast withdrawal request, err: {:?}", err);
                                                    }
                                                }
                                            }
                                            Err(err) => {
                                                error!("Warning: Failed to send BTC server withdrawal request, {:?}", err);
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    // TODO: make this a future
                    // await the fcu call rx for SYNCING, then wait for a VALID response
                    loop {
                        // send the new update to the engine, this will trigger the engine
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
                                error!(target: "consensus::authority", ?err, "Autoseal fork choice update failed");
                                return ()
                            }
                        }
                    }

                    // seal the block
                    let block = Block {
                        header: new_header.clone().unseal(),
                        body: transactions,
                        ommers: vec![],
                        withdrawals: None,
                    };
                    let sealed_block = block.seal_slow();

                    let sealed_block_with_senders =
                        SealedBlockWithSenders::new(sealed_block, senders)
                            .expect("senders are valid");

                    // update canon chain for rpc
                    self.client.set_canonical_head(new_header.clone());
                    self.client.set_safe(new_header.clone());
                    self.client.set_finalized(new_header.clone());

                    info!(target: "consensus::authority", header=?sealed_block_with_senders.hash(), "sending block notification");

                    let chain = Arc::new(Chain::new(vec![(sealed_block_with_senders, post_state)]));

                    // send block notification
                    let _ = self
                        .canon_state_notification
                        .send(reth_provider::CanonStateNotification::Commit { new: chain });
                }
                Err(err) => {
                    warn!(target: "consensus::authority", ?err, "failed to execute block")
                }
            }

            self.pipe_line_events = events;
        }
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
