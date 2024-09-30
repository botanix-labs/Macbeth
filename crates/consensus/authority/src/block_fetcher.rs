use std::time::{Duration, Instant};

use crate::{engine_util, utils::is_active_sync_in_progress, AuthorityConsensus, Storage};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_blockchain_tree_api::BlockchainTreeEngine;
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_eth_wire::NewBlock;
use reth_evm::execute::BlockExecutorProvider;
use reth_network::{message::NewBlockMessageWithPeerId, NetworkHandle};
use reth_network_api::{test_utils::PeersHandleProvider, PeerId};
use reth_node_ethereum::EthEngineTypes;
use reth_primitives::{SealedBlockWithSenders, B256};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use ruint::Uint;
use tendermint_light_client::instance::Instance;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{error, info, warn};

pub struct BlockFetcherTask<EF, BF, DB> {
    /// Authority consensus
    consensus: AuthorityConsensus,
    /// Channel to receive new blocks
    block_import_rx: UnboundedReceiver<NewBlockMessageWithPeerId>,
    /// Channel to send new blocks to the engine
    to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
    /// Consensus cache
    storage: Storage<EF, BF, DB>,
    /// Network Handle
    network_handle: NetworkHandle,
    /// Light client
    light_client: Instance,
}

impl<EF, BF, DB> BlockFetcherTask<EF, BF, DB>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        consensus: AuthorityConsensus,
        block_import_rx: UnboundedReceiver<NewBlockMessageWithPeerId>,
        to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
        storage: Storage<EF, BF, DB>,
        network_handle: NetworkHandle,
        light_client: Instance,
    ) -> Self {
        Self { consensus, block_import_rx, to_engine, storage, network_handle, light_client }
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
            info!(target: "consensus::authority", "Received new block from peer {:?}", block_hash);

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
                warn!(target: "consensus::authority", "Received block is already in the chain");
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

            let _latest_trusted =
                self.light_client.latest_trusted().expect("to get latest trusted");
            match self.light_client.light_client.verify_to_highest(&mut self.light_client.state) {
                Ok(_) => (),
                Err(e) => {
                    warn!(target: "consensus::authority", "Failed to verify block: {:?}", e);
                    self.ban_peer(peer_id);
                    continue;
                }
            };

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
