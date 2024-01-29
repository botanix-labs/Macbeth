use crate::engine_util;

use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_primitives::{SealedBlockWithSenders, TransactionSigned};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, Chain, StateProviderFactory};

use crate::Storage;
use client::BtcServerClient;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_btc_wallet::block_source::MempoolSpace;
use reth_network::message::NewBlockMessage;
use reth_primitives::ChainSpec;
use reth_provider::CanonStateNotificationSender;

use std::sync::Arc;
use tokio::sync::{
    mpsc::{error::TryRecvError, UnboundedReceiver, UnboundedSender},
    RwLock,
};

use tracing::{debug, error, info};

pub struct BlockFetcherTask<Client> {
    chain_spec: Arc<ChainSpec>,
    block_import_rx: UnboundedReceiver<NewBlockMessage>,
    to_engine: UnboundedSender<BeaconEngineMessage>,
    /// Used to notify consumers of new blocks
    canon_state_notification: CanonStateNotificationSender,
    /// Btc Server client
    btc_server: BtcServerClient<tonic::transport::Channel>,
    /// bitcoin block source
    bitcoin_block_source: MempoolSpace,
    /// Consensus cache
    storage: Storage<Client>,
    /// Recent bitcoin header
    bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
}

impl<Client> BlockFetcherTask<Client>
where
    Client: BlockchainTreeEngine
        + BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + Clone
        + 'static,
{
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        block_import_rx: UnboundedReceiver<NewBlockMessage>,
        to_engine: UnboundedSender<BeaconEngineMessage>,
        canon_state_notification: CanonStateNotificationSender,
        btc_server: BtcServerClient<tonic::transport::Channel>,
        bitcoin_block_source: MempoolSpace,
        storage: Storage<Client>,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    ) -> Self {
        Self {
            chain_spec,
            block_import_rx,
            to_engine,
            canon_state_notification,
            btc_server,
            bitcoin_block_source,
            storage,
            bitcoin_block_header,
        }
    }

    pub async fn start_task(&mut self) {
        loop {
            let new_block = match self.block_import_rx.try_recv() {
                Ok(b) => b,
                Err(error) => match error {
                    TryRecvError::Empty => {
                        debug!(target: "consensus::authority", "No new blocks from peers");
                        continue
                    }
                    TryRecvError::Disconnected => {
                        debug!(target: "consensus::authority", "Block import channel disconnected");
                        continue
                    }
                },
            };

            let block = new_block.block.block.clone();
            info!(target: "consensus::authority", ?block, "Recieved new block from peer");

            // Seal the block
            let sealed_block = block.clone().seal_slow();
            // Notify the engine of the new block
            let _payload_status = match engine_util::send_beacon_new_payload(
                sealed_block.clone(),
                self.to_engine.clone(),
            )
            .await
            {
                Ok(payload) => payload,
                Err(err) => {
                    error!(target: "consensus::authority", ?err, "Block import failed to send new payload to engine");
                    continue
                }
            };

            let recent_bitcoin_block_header = self.bitcoin_block_header.read().await.clone();
            let mut storage = self.storage.write().await;

            match storage.execute_imported_block(
                self.chain_spec.clone(),
                sealed_block.clone(),
                recent_bitcoin_block_header,
            ) {
                Ok(bundle_state) => {
                    let senders =
                        TransactionSigned::recover_signers(&block.body, block.body.len()).unwrap();
                    let sealed_block_with_senders =
                        SealedBlockWithSenders::new(sealed_block.clone(), senders)
                            .expect("senders are valid");
                    // Process Botanix specific logs
                    match crate::utils::process_reciepts(
                        &self.bitcoin_block_source,
                        &mut self.btc_server.clone(),
                        &bundle_state,
                        false,
                    )
                    .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            error!(target: "consensus::authority", ?e, "Failed to process botanix log");
                            continue
                        }
                    }
                    // Notify engine api about new FCU
                    engine_util::send_fork_choice_update_payload(
                        sealed_block.clone().hash,
                        self.to_engine.clone(),
                    )
                    .await
                    // TODO remove unwrap
                    .unwrap();

                    // update canon chain for rpc
                    // TODO do we need to insert the block here?
                    storage.client.set_canonical_head(sealed_block.header.clone());
                    storage.client.set_safe(sealed_block.header.clone());
                    storage.client.set_finalized(sealed_block.header.clone());

                    drop(storage);

                    let chain = Arc::new(Chain::new(vec![sealed_block_with_senders], bundle_state));

                    info!(target: "consensus::authority", "sending block notification to block chain tree");
                    // send block notification
                    let _ = self
                        .canon_state_notification
                        .send(reth_provider::CanonStateNotification::Commit { new: chain });
                }
                Err(err) => {
                    error!(target: "consensus::authority", ?err, "Failed to exectute block recieved by peer");
                }
            }
        }
    }
}
