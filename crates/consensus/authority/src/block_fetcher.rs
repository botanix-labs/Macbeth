use std::{sync::Arc, time::Duration};

use crate::{
    engine_util,
    excecution_utils::authority_execution_utils::execute_imported_block,
    utils::{bloom_contains_pegin, call_notify_pegin, is_active_sync_in_progress},
    utxo_sync::{UTXOSync, UTXOSyncEngine},
    AuthorityConsensus, LightCBFTClientBuilder, Storage,
};

use comet_bft_rpc::{Client, HttpCometBFTRpcClientFactory};
use tendermint_light_client::instance::Instance;
use tendermint_rpc::HttpClient;
use bitcoin::hashes::{sha256, Hash};
use btcserverlib::extended_client::BtcServerExtendedClient;
use client::{FinalizeSignerRequest, Output};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_blockchain_tree_api::{BlockValidationKind, BlockchainTreeEngine};
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_evm::execute::BlockExecutorProvider;
use reth_network::{frost::manager::ToFrostManager, message::NewBlockMessage, NetworkHandle};
use reth_network_p2p::{full_block::FullBlockClient, BodiesClient, HeadersClient};
use reth_node_api::EngineTypes;
use reth_primitives::{header_ext::HeaderExt, SealedBlockWithSenders, TransactionSigned};
use reth_provider::{
    BlockReaderIdExt, CanonChainTracker, CanonStateNotificationSender, Chain, StateProviderFactory,
};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    RwLock,
};
use tracing::{error, info, warn};

pub struct BlockFetcherTask<EF, BF, DB, NetworkClient, ToFrostMan> {
    /// Authority consensus
    consensus: AuthorityConsensus,
    /// Channel to recieve new blocks
    block_import_rx: UnboundedReceiver<NewBlockMessage>,
    /// Channel to send new blocks to the engine
    to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
    /// Used to notify consumers of new blocks
    canon_state_notification: CanonStateNotificationSender,
    /// Btc Server client
    btc_server: Option<BtcServerExtendedClient>,
    /// Consensus cache
    storage: Storage<EF, BF, DB>,
    /// Recent finalize bitcoin block checkpoint.
    bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    /// Network Client, used to create [FullBlockClient]
    network_client: NetworkClient,
    /// Network Handle, used to create [FullBlockClient]
    network_handle: NetworkHandle,
    /// Utxo set sync controller
    /// Rpc servers will not have a utxo sync engine
    utxo_sync: Option<UTXOSyncEngine<EF, BF, DB, ToFrostMan>>,
    /// Light client
    light_client: Instance,
}

impl<EF, BF, DB, NetworkClient, ToFrostMan> BlockFetcherTask<EF, BF, DB, NetworkClient, ToFrostMan>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    NetworkClient: HeadersClient + BodiesClient + Clone + Unpin + 'static,
    ToFrostMan: ToFrostManager + Clone + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        consensus: AuthorityConsensus,
        block_import_rx: UnboundedReceiver<NewBlockMessage>,
        to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
        canon_state_notification: CanonStateNotificationSender,
        btc_server: Option<BtcServerExtendedClient>,
        storage: Storage<EF, BF, DB>,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        network_client: NetworkClient,
        network_handle: NetworkHandle,
        utxo_sync: Option<UTXOSyncEngine<EF, BF, DB, ToFrostMan>>,
        light_client: Instance,
    ) -> Self {
        Self {
            consensus,
            block_import_rx,
            to_engine,
            canon_state_notification,
            btc_server,
            storage,
            bitcoin_block_header,
            network_client,
            network_handle,
            utxo_sync,
            light_client,
        }
    }

    pub async fn start_task(&mut self) {
        // only a federation node has a btc_server
        let is_fed_node = self.btc_server.is_some();
        let consensus = Arc::new(self.consensus.clone());
        loop {
            // ensure the node is not syncing
            if is_active_sync_in_progress(&self.network_handle) {
                warn!(target: "consensus::authority", "Node is still syncing, block fetcher task
            is awaiting fully synced status ...");
                tokio::time::sleep(Duration::from_millis(500)).await;
                return;
            }

            let new_block = match self.block_import_rx.recv().await {
                Some(b) => b,
                None => {
                    info!(target: "consensus::authority",
                        "block fetcher task shutting down (channel closed)",
                    );
                    return;
                }
            };
            let cbft_block = self
                .light_client
                .get_or_fetch_block(new_block.number().try_into().unwrap())
                .unwrap();
            self.light_client.trust_block(&cbft_block);

            let latest_trusted = self.light_client.latest_trusted().expect("to get latest trusted");
            self.light_client.light_client.verify_to_highest(&mut self.light_client.state).unwrap();
            info!(">>>>>>>>>>> Latest trusted: {:?}", latest_trusted);

            let block = new_block.block.block.clone();
            let app_hash = cbft_block.signed_header.header.app_hash.as_bytes();
            if app_hash != &block.parent_hash.0 {
                warn!(target: "consensus::authority", "App hash mismatch");
                warn!(target: "consensus::authority", "Expecting {:?}, got {:?}", block.hash_slow(), B256::from_slice(app_hash));
                continue;
            }

            info!(target: "consensus::authority", "Recieved new block from peer {:?}", block.header.hash_slow());
            let client = self.storage.client.clone();

            let storage = self.storage.write().await;
            info!(">>>>>>>>>>> storage");
            let aggregate_public_key = storage.aggregate_public_key.clone();
            let executor_factory = storage.executor_factory.clone();
            let authorities = storage.authorities.clone();
            let genesis_authorities = storage.genesis_authorities.clone();

            let best_block = client.best_block_number().expect("best block number exists");
            let best_hash = client
                .block_hash(best_block)
                .expect("best block hash exists")
                .unwrap_or_else(|| {
                    panic!("best block hash is valid");
                });
            if block.header.hash_slow() == best_hash {
                warn!(target: "consensus::authority", "Recieved block is already in the chain");
                continue;
            }
            // Seal the block
            let sealed_block = block.clone().seal_slow();
            info!(">>>>>>>>>>> sealed_block");

            // if is_fed_node && storage.aggregate_public_key.is_none() {
            //     warn!(target: "consensus::authority", "Do not have aggregate public key in
            // memory, skipping block import");     continue;
            // }

            // Drop the storage lock as soon as possible to allow other tasks to run
            drop(storage);

            // TODO we hang here
            // Notify the engine of the new block
            // let _payload_status = match engine_util::send_beacon_new_payload(
            //     sealed_block.clone(),
            //     self.to_engine.clone(),
            // )
            // .await
            // {
            //     Ok(payload) => payload,
            //     Err(err) => {
            //         error!(target: "consensus::authority", ?err, "Block import failed to send new
            // payload to engine");
            //         continue;
            //     }
            // };
            // info!(">>>>>>>>>>> send_beacon_new_payload");
            // TODO Should be handling payload status here

            let sealed_block_with_peg = match execute_imported_block(
                &self.consensus,
                sealed_block.clone(),
                &client,
                &executor_factory,
                aggregate_public_key.as_ref(),
                &authorities,
                &genesis_authorities,
            ) {
                Ok(sealed_block_with_peg) => sealed_block_with_peg,
                Err(err) => {
                    error!(target: "consensus::authority", ?err, "Failed to exectute block
            recieved by peer");
                    continue;
                }
            };
            info!(">>>>>>>>>>> execute_imported_block");

            let sealed_block_with_senders = sealed_block_with_peg.block();
            let header = sealed_block.header.clone();

            // Notify engine api about new FCU
            match engine_util::send_fork_choice_update_payload(
                sealed_block_with_senders.clone().hash(),
                self.to_engine.clone(),
            )
            .await
            {
                Ok(_) => (),
                Err(e) => {
                    error!(target: "consensus::authority", ?e, "Failed to notify engine of new
            FCU");
                    continue;
                }
            };
            info!(">>>>>>>>>>> send_fork_choice_update_payload");
            // update canon chain for rpc
            client.set_canonical_head(header);
            client.set_safe(sealed_block.header.clone());
            client.set_finalized(sealed_block.header.clone());
            info!(">>>>>>>>>>> set_canonical_head");
        }
    }
}
