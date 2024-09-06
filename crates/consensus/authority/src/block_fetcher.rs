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
    block_import_rx: UnboundedReceiver<NewBlockMessageWithPeerId>,
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
        block_import_rx: UnboundedReceiver<NewBlockMessageWithPeerId>,
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

    pub fn ban_peer(&self, peer_id: PeerId) {
        self.network_handle
            .peers_handle()
            .reputation_change(peer_id, reth_network_api::ReputationChangeKind::BadBlock);
    }

    pub async fn start_task(&mut self) {
        loop {
            // ensure the node is not syncing
            if is_active_sync_in_progress(&self.network_handle) {
                warn!(target: "consensus::authority", "Node is still syncing, block fetcher task
            is awaiting fully synced status ...");
                tokio::time::sleep(Duration::from_millis(250)).await;
                return;
            }

            let new_block_with_peer_id = match self.block_import_rx.recv().await {
                Some(b) => b,
                None => {
                    info!(target: "consensus::authority",
                        "block fetcher task shutting down (channel closed)",
                    );
                    return;
                }
            };

            let new_block = new_block_with_peer_id.message;
            let peer_id = new_block_with_peer_id.peer_id;

            let block = new_block.block.block.clone();
            let block_hash = block.header.hash_slow();
            info!(target: "consensus::authority", "Recieved new block from peer {:?}", block_hash);

            // This shouldn't happen but check that the block we are importing is not already in the
            // chain
            let client = self.storage.client.clone();
            let best_block = client.best_block_number().expect("best block number exists");
            let best_hash = client
                .block_hash(best_block)
                .expect("best block hash exists")
                .unwrap_or_else(|| {
                    panic!("best block hash is valid");
                });
            if block_hash == best_hash {
                warn!(target: "consensus::authority", "Recieved block is already in the chain");
                continue;
            }

            let start_time = Instant::now();
            // skip block adding if this fails
            let block_number = match new_block.number().try_into() {
                Ok(block_number) => block_number,
                Err(_) => {
                    warn!(target: "consensus::authority", "Block number does not fit in u64");
                    self.ban_peer(peer_id);
                    continue;
                }
            };
            let cbft_block = match self.light_client.get_or_fetch_block(block_number) {
                Ok(cbft_block) => cbft_block,
                Err(e) => {
                    warn!(target: "consensus::authority", "Failed to get or fetch block from light client primary source: {:?}", e);
                    self.ban_peer(peer_id);
                    continue;
                }
            };
            self.light_client.trust_block(&cbft_block);

            let latest_trusted = self.light_client.latest_trusted().expect("to get latest trusted");
            match self.light_client.light_client.verify_to_highest(&mut self.light_client.state) {
                Ok(_) => (),
                Err(e) => {
                    warn!(target: "consensus::authority", "Failed to verify block: {:?}", e);
                    self.ban_peer(peer_id);
                    continue;
                }
            };
            // TODO should ban peer if verification fails
            // self.network_handle.peers_handle().reputation_change(peer_id, kind)

            let app_hash = cbft_block.signed_header.header.app_hash.as_bytes();
            if app_hash != &block.parent_hash.0 {
                warn!(target: "consensus::authority", "App hash mismatch");
                warn!(target: "consensus::authority", "Expecting {:?}, got {:?}", block.hash_slow(), B256::from_slice(app_hash));
                self.ban_peer(peer_id);
                continue;
            }
            let end_time = Instant::now();
            info!("light_client.get_or_fetch_block took {:?}", end_time.duration_since(start_time));

            // Seal the block
            let sealed_block = block.clone().seal_slow();
            let senders = new_block.block.block.senders().unwrap();
            if senders.len() != new_block.block.block.body.len() {
                warn!(target: "consensus::authority", "Senders length does not match transactions length");
                self.ban_peer(peer_id);
                continue;
            }
            let sealed_block_with_senders = SealedBlockWithSenders::new(sealed_block, senders)
                .expect("senders length to match transactions length");
            let header = sealed_block_with_senders.header.clone();
            assert!(header.hash_slow() == block_hash);

            // Notify engine api about new FCU
            let start_time = Instant::now();

            match engine_util::send_fork_choice_update_payload(
                header.hash(),
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
            let end_time = Instant::now();
            info!("send_fork_choice_update_payload took {:?}", end_time.duration_since(start_time));
            // update canon chain for rpc
            client.set_canonical_head(header.clone());
            client.set_safe(header.clone());
            client.set_finalized(header.clone());

            self.network_handle.announce_block(NewBlock { block, td: Uint::ZERO }, block_hash);
        }
    }
}
