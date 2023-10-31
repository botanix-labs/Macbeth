use crate::Storage;
use botanix_lib::mint_validation::{
    parse_pegin_reth_log_topic, parse_pegout_reth_log_topic, GenesisContractEvents,
};

use btc_wallet::block_source::{BlockSource, MempoolSpace};
use futures_util::{future::BoxFuture, FutureExt};
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
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
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
    epoch: EpochManager,
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

impl<Client, Pool: TransactionPool> BlockProductionTask<Client, Pool> {
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
        }
    }

    /// Sets the pipeline events to listen on.
    pub fn set_pipeline_events(&mut self, events: UnboundedReceiverStream<PipelineEvent>) {
        self.pipe_line_events = Some(events);
    }
}

impl<Client, Pool> Future for BlockProductionTask<Client, Pool>
where
    Client: StateProviderFactory + CanonChainTracker + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    <Pool as TransactionPool>::Transaction: IntoRecoveredTransaction,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // this drives block production
        loop {
            // TODO this piece needs to get replaced by polling the epoch manager
            // if let Poll::Ready(transactions) = this.miner.poll(&this.pool, cx) {
            //     info!("Adding to the list of transctions, {:?}, {:?}", transactions,
            // this.queued);     // miner returned a set of transaction that we feed to
            // the producer     this.queued.push_back(transactions.clone());
            //     let mining_pool = this.pool.clone();
            //     mining_pool.remove_transactions(
            //         transactions.iter().map(|tx| tx.hash().to_owned()).collect(),
            //     );
            // }

            // If insert task is not none executinon of async task is on going
            if this.insert_task.is_none() {
                if this.queued.is_empty() {
                    info!("Txs list is empty, skipping");
                    // nothing to insert
                    break
                }

                // ready to queue in new insert task
                let storage = this.storage.clone();
                let transactions = this.queued.pop_front().expect("not empty");

                let to_engine = this.to_engine.clone();
                let client = this.client.clone();
                let chain_spec = Arc::clone(&this.chain_spec);
                let pool = this.pool.clone();
                let events = this.pipe_line_events.take();
                let canon_state_notification = this.canon_state_notification.clone();
                let mut btc_server = this.btc_server.clone();
                let bitcoin_block_header = this.bitcoin_block_header.clone();
                let block_source =
                    MempoolSpace::new(this.bitcoin_block_source_address.clone().to_string());

                // TOOD(armins) find a way to remove this clone
                let secp = this.secp.clone();
                let sk = this.sk;
                // Create the mining future that creates a block, notifies the engine that drives
                // the pipeline
                this.insert_task = Some(Box::pin(async move {
                    let recent_block_header = bitcoin_block_header.read().await.clone();
                    let mut storage = storage.write().await;

                    let (transactions, senders): (Vec<_>, Vec<_>) = transactions
                        .into_iter()
                        .map(|tx| {
                            let recovered = tx.to_recovered_transaction();
                            let signer = recovered.signer();
                            (recovered.into_signed(), signer)
                        })
                        .unzip();

                    // execute the new block
                    let substate = SubState::new(State::new(client.latest().unwrap()));
                    let mut executor = Executor::new(Arc::clone(&chain_spec), substate);
                    match storage.build_and_execute(
                        transactions.clone(),
                        &mut executor,
                        chain_spec,
                        recent_block_header,
                        // TODO(armins) read vote in as param
                        &None,
                        &sk,
                        &secp,
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
                                                    utxo_txid: pegin_data
                                                        .meta
                                                        .outpoint
                                                        .txid
                                                        .to_string(),
                                                    utxo_vout: pegin_data.meta.outpoint.vout,
                                                    eth_address: hex::encode(
                                                        pegin_data.meta.address.to_vec(),
                                                    ),
                                                    output: bitcoin::consensus::serialize(
                                                        &pegin_data
                                                            .meta
                                                            .tx
                                                            .output
                                                            .get(
                                                                pegin_data.meta.outpoint.vout
                                                                    as usize,
                                                            )
                                                            .unwrap(),
                                                    ),
                                                    nonce: pegin_data.nonce,
                                                };
                                                btc_server.notify_pegin(request).await.unwrap();
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

                                                match btc_server.make_tx(request).await {
                                                    Ok(response) => {
                                                        let raw_tx = response.into_inner().tx;
                                                        info!("Pegout tx from btc signer service {:?}", raw_tx.clone());

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
                                let _ = to_engine.send(BeaconEngineMessage::ForkchoiceUpdated {
                                    state,
                                    payload_attrs: None,
                                    tx,
                                });
                                debug!(target: "consensus::auto", ?state, "Sent fork choice update");

                                match rx.await.unwrap() {
                                    Ok(fcu_response) => {
                                        match fcu_response.forkchoice_status() {
                                            ForkchoiceStatus::Valid => break,
                                            ForkchoiceStatus::Invalid => {
                                                error!(target: "consensus::auto", ?fcu_response, "Forkchoice update returned invalid response");
                                                return None
                                            }
                                            ForkchoiceStatus::Syncing => {
                                                debug!(target: "consensus::auto", ?fcu_response, "Forkchoice update returned SYNCING, waiting for VALID");
                                                // wait for the next fork choice update
                                                continue
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        error!(target: "consensus::auto", ?err, "Autoseal fork choice update failed");
                                        return None
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
                            client.set_canonical_head(new_header.clone());
                            client.set_safe(new_header.clone());
                            client.set_finalized(new_header.clone());

                            info!(target: "consensus::auto", header=?sealed_block_with_senders.hash(), "sending block notification");

                            let chain =
                                Arc::new(Chain::new(vec![(sealed_block_with_senders, post_state)]));

                            // send block notification
                            let _ = canon_state_notification
                                .send(reth_provider::CanonStateNotification::Commit { new: chain });
                        }
                        Err(err) => {
                            warn!(target: "consensus::auto", ?err, "failed to execute block")
                        }
                    }

                    events
                }));
            }

            if let Some(mut fut) = this.insert_task.take() {
                match fut.poll_unpin(cx) {
                    Poll::Ready(events) => {
                        this.pipe_line_events = events;
                    }
                    Poll::Pending => {
                        this.insert_task = Some(fut);
                        break
                    }
                }
            }
        }

        Poll::Pending
    }
}

impl<Client, Pool: TransactionPool> std::fmt::Debug for BlockProductionTask<Client, Pool> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockProductionTask").finish_non_exhaustive()
    }
}
