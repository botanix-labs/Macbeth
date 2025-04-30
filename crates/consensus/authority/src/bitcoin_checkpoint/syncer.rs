// TODO: Better solution would be to trigger sync using Bitcoin ZMQ new block event

use super::chain::BitcoinCheckpointsChain;
use super::checkpoint::BitcoinCheckpoint;
use super::error::BitcoinCheckpointError;
use std::sync::{Arc, Mutex};
use std::time::Duration;

macro_rules! map_rpc_error {
    ($target:expr, $method:ident ( $($args:tt)* )) => {{
        $target.$method($($args)*).map_err(|error| {
            BitcoinCheckpointError::RpcError {
                error,
                procedure_name: stringify!($method).to_string(),
            }
        })
    }};
}

const SLEEP: Duration = Duration::from_secs(10);

pub struct BitcoinCheckpointsChainSynchronizer<R> {
    rpc: R,
    checkpoints_chain: Arc<BitcoinCheckpointsChain>,
    // RPC is using u64 for block height for some reason, so we use it as well to avoid casting
    last_synced_height: Option<u64>,
}

impl<R> BitcoinCheckpointsChainSynchronizer<R>
where
    R: reth_btc_wallet::bitcoind::RpcApiExt + 'static,
{
    pub fn new(checkpoints_chain: Arc<BitcoinCheckpointsChain>, rpc: R) -> Self {
        // Calculate the last synced height based on the most recent checkpoint
        // in chain and the lowest confirmation depth
        let last_synced_height = checkpoints_chain
            .recent_height()
            .map(|height| height as u64 + checkpoints_chain.lowest_confirmations_depth() as u64);

        Self { rpc, checkpoints_chain, last_synced_height }
    }

    /// It will return StaleBlockAdded error if a new block arrives during sync.
    fn sync_new_blocks(&mut self) -> Result<(), BitcoinCheckpointError> {
        let tip_height = map_rpc_error!(self.rpc, get_block_count())?;

        let last_synced_height = self.last_synced_height.unwrap_or_default();

        // Don't sync if we're at the same height
        if tip_height <= last_synced_height {
            return Ok(());
        }

        // Figure out how many blocks we need to sync
        let chain_size_limit = self.checkpoints_chain.size_limit() as u64;
        let mut blocks_to_sync_count = tip_height - last_synced_height;
        if blocks_to_sync_count > chain_size_limit {
            blocks_to_sync_count = chain_size_limit;
        }

        // Calculate the range of block heights to sync
        let lowest_confirmations_depth = self.checkpoints_chain.lowest_confirmations_depth() as u64;

        // To keep chain consistency we need to push from lowest to highest checkpoint
        let from_height = tip_height - lowest_confirmations_depth + blocks_to_sync_count;
        let to_height = tip_height - lowest_confirmations_depth;

        for height in from_height..=to_height {
            let confirmed_hash = map_rpc_error!(self.rpc, get_block_hash(height))?;

            let header = map_rpc_error!(self.rpc, get_block_header(&confirmed_hash))?;

            // Create and push the checkpoint
            let bitcoin_checkpoint = BitcoinCheckpoint::new(header, height as u32);

            self.checkpoints_chain.push(bitcoin_checkpoint)?;
        }

        // Update the last height we've seen
        self.last_synced_height = Some(tip_height);

        // TODO: return synced hashes and heights for logging
        Ok(())
    }

    /// Run the synchronizer forever.
    pub async fn sync(self) {
        // We need interior mutability so that we can move `self` into spawn_blocking
        let syncer = Arc::new(Mutex::new(self));

        loop {
            // Call `sync_new_blocks()` on the blocking pool
            let syncer_clone = Arc::clone(&syncer);
            let result = tokio::task::spawn_blocking(move || {
                let mut syncer = syncer_clone.lock().unwrap();
                syncer.sync_new_blocks()
            })
            .await
            .expect("spawn_blocking task panicked");

            match result {
                Ok(_) => tracing::info!("bitcoin checkpoints synced"),
                Err(e) => tracing::warn!("bitcoin checkpoints sync failed: {e}"),
            }

            tokio::time::sleep(SLEEP).await;
        }
    }
}
