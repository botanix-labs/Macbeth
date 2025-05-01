// TODO: Better solution would be to trigger sync using Bitcoin ZMQ new block event

use super::chain::BitcoinCheckpointsChain;
use super::checkpoint::BitcoinCheckpoint;
use super::error::BitcoinCheckpointError;
use bitcoin::block::BlockHash as BitcoinBlockHash;
use std::fmt::Display;
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

#[derive(Debug)]
struct SyncedCheckpointInfo {
    height: u32,
    hash: BitcoinBlockHash,
}

impl From<&BitcoinCheckpoint> for SyncedCheckpointInfo {
    fn from(checkpoint: &BitcoinCheckpoint) -> Self {
        Self { height: checkpoint.height, hash: checkpoint.hash }
    }
}

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
            .map(|height| height as u64 + checkpoints_chain.lowest_confirmation_depth() as u64);

        Self { rpc, checkpoints_chain, last_synced_height }
    }

    /// It will return StaleBlockAdded error if a new block arrives during sync.
    fn sync_new_blocks(&mut self) -> Result<Vec<SyncedCheckpointInfo>, BitcoinCheckpointError> {
        let tip_height = map_rpc_error!(self.rpc, get_block_count())?;

        let last_synced_height = self.last_synced_height.unwrap_or_default();

        // Don't sync if we're at the same height
        if tip_height <= last_synced_height {
            return Ok(Vec::new());
        }

        // Is the chain too young to have any blocks with the
        // required lowest confirmation depth?
        let lowest_confirmation_depth = self.checkpoints_chain.lowest_confirmation_depth() as u64;
        if tip_height < lowest_confirmation_depth {
            // Not enough blocks yet.
            // Just remember the new tip and return.
            self.last_synced_height = Some(tip_height);

            return Ok(Vec::new());
        }

        // Total confirmed blocks currently available
        let confirmed_available = tip_height + 1 - lowest_confirmation_depth;

        // How many of them have we already synced?
        let confirmed_already =
            last_synced_height.saturating_add(1).saturating_sub(lowest_confirmation_depth);
        let need_to_sync = confirmed_available.saturating_sub(confirmed_already);

        if need_to_sync == 0 {
            self.last_synced_height = Some(tip_height);

            return Ok(Vec::new());
        }

        // How many blocks we need to sync (limited by the chain size)
        let chain_size_limit = self.checkpoints_chain.size_limit() as u64;
        let blocks_to_sync = need_to_sync.min(chain_size_limit);

        let top_confirmed_height = tip_height - (lowest_confirmation_depth - 1);

        // We push from oldest to newest, so we start `blocks_to_sync`−1 below the top.
        let from_height = top_confirmed_height - (blocks_to_sync - 1);
        let to_height = top_confirmed_height;

        let mut synced_checkpoints = Vec::new();
        for height in from_height..=to_height {
            let confirmed_hash = map_rpc_error!(self.rpc, get_block_hash(height))?;
            let header = map_rpc_error!(self.rpc, get_block_header(&confirmed_hash))?;

            // Create, report and push the checkpoint
            let bitcoin_checkpoint = BitcoinCheckpoint::new(header, height as u32);

            synced_checkpoints.push(SyncedCheckpointInfo::from(&bitcoin_checkpoint));

            self.checkpoints_chain.push(bitcoin_checkpoint)?;
        }

        // Update the last height we've seen
        self.last_synced_height = Some(tip_height);

        Ok(synced_checkpoints)
    }

    /// Run the synchronizer forever.
    pub async fn sync(self) {
        // We need interior mutability so that we can move `self` into spawn_blocking
        let syncer = Arc::new(Mutex::new(self));

        loop {
            // Sync bitcoin checkpoints to the tip in blocking task
            let syncer_clone = Arc::clone(&syncer);
            let result = tokio::task::spawn_blocking(move || {
                let mut syncer = syncer_clone.lock().unwrap();
                syncer.sync_new_blocks()
            })
            .await
            .expect("spawned blocking task failed to sync bitcoin checkpoints");

            match result {
                Ok(synced_checkpoints) => {
                    tracing::info!(
                        ?synced_checkpoints,
                        "Asynced task synced {} bitcoin checkpoints",
                        synced_checkpoints.len()
                    )
                }
                Err(e) => tracing::warn!("Async task failed to sync bitcoin checkpoints: {e}"),
            }

            tokio::time::sleep(SLEEP).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{
        block::Header as BitcoinHeader, hashes::Hash, BlockHash as BitcoinBlockHash, TxMerkleNode,
    };

    use mockall::{mock, predicate::*};
    use reth_btc_wallet::bitcoind::jsonrpc::serde;

    mod sync_new_blocks {
        use super::*;

        #[test]
        fn test_no_new_blocks_does_nothing() {
            let chain =
                Arc::new(BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create valid chain"));

            let mut mock = MockRpc::new();
            mock.expect_get_block_count().returning(|| Ok(100));

            let mut syncer = BitcoinCheckpointsChainSynchronizer::new(Arc::clone(&chain), mock);
            syncer.last_synced_height = Some(100);

            syncer.sync_new_blocks().expect("sync new blocks");

            assert_eq!(chain.len(), 0);
        }
        #[test]
        fn test_new_blocks_fewer_than_limit_fetches_exact_delta() {
            // limit = 7, lowest_conf_depth = 4 -> heights 98-102
            let chain =
                Arc::new(BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create valid chain"));

            let mut mock = MockRpc::new();
            mock.expect_get_block_count().returning(|| Ok(105));

            expect_header_chain(&mut mock, 98..=102, BitcoinBlockHash::all_zeros());

            let mut syncer = BitcoinCheckpointsChainSynchronizer::new(Arc::clone(&chain), mock);
            syncer.last_synced_height = Some(100);

            syncer.sync_new_blocks().expect("sync new blocks");

            assert_eq!(chain.len(), 5);
        }

        #[test]
        fn test_blocks_truncated_to_limit() {
            // limit = 7 -> heights 131-137
            let chain =
                Arc::new(BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create valid chain"));

            let mut mock = MockRpc::new();
            mock.expect_get_block_count().returning(|| Ok(140));

            expect_header_chain(&mut mock, 131..=137, BitcoinBlockHash::all_zeros());

            let mut syncer = BitcoinCheckpointsChainSynchronizer::new(Arc::clone(&chain), mock);
            syncer.last_synced_height = Some(100);

            syncer.sync_new_blocks().expect("sync new blocks");

            assert_eq!(chain.len(), 7);
        }

        #[test]
        fn test_rpc_error_mapping() {
            let chain =
                Arc::new(BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create valid chain"));

            let mut mock = MockRpc::new();
            mock.expect_get_block_count()
                .return_once(|| Err(reth_btc_wallet::bitcoind::JsonRPCError::UnexpectedStructure));

            let mut syncer = BitcoinCheckpointsChainSynchronizer::new(chain, mock);

            let result = syncer.sync_new_blocks();

            assert!(
                matches!(result, Err(BitcoinCheckpointError::RpcError { procedure_name, .. }) if procedure_name == "get_block_count")
            );
        }

        #[test]
        fn test_stale_block_error() {
            let chain =
                Arc::new(BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create valid chain"));

            // Preload height-1 checkpoint, so the next height-1 push will be "stale"
            let header = create_header(BitcoinBlockHash::all_zeros());

            chain.push(BitcoinCheckpoint::new(header, 1)).unwrap();

            let mut mock = MockRpc::new();

            // Tip=5 (enough to sync)
            // Syncer will fetch heights 1-2
            // Height 1 will collide, and we get `StaleBlockAdded`
            mock.expect_get_block_count().returning(|| Ok(5));

            let h1 = BitcoinBlockHash::from_byte_array([1u8; 32]);
            let h2 = BitcoinBlockHash::from_byte_array([2u8; 32]);

            mock.expect_get_block_hash().with(eq(1u64)).returning(move |_| Ok(h1));
            mock.expect_get_block_header()
                .with(eq(h1))
                .returning(|_| Ok(create_header(BitcoinBlockHash::all_zeros())));

            mock.expect_get_block_hash().with(eq(2u64)).returning(move |_| Ok(h2));
            mock.expect_get_block_header().with(eq(h2)).returning(move |_| Ok(create_header(h1)));

            let mut syncer = BitcoinCheckpointsChainSynchronizer::new(chain, mock);
            syncer.last_synced_height = Some(1);

            let result = syncer.sync_new_blocks();

            assert!(matches!(result, Err(BitcoinCheckpointError::StaleBlockAdded { .. })));
        }
    }

    // Mock Bitcoin RPC client

    mock! {
        pub Rpc {
            fn get_block_count(&self)
                -> Result<u64, reth_btc_wallet::bitcoind::JsonRPCError>;

            fn get_block_hash(&self, height: u64)
                -> Result<BitcoinBlockHash, reth_btc_wallet::bitcoind::JsonRPCError>;

            fn get_block_header(&self, hash: &BitcoinBlockHash)
                -> Result<BitcoinHeader, reth_btc_wallet::bitcoind::JsonRPCError>;
        }
    }

    // Mockall doesn't allow to mock `call` method because it has a generic parameter without 'static lifetime.
    // So to satisfy the `RpcApi` trait, we need to implement the `call` method directly in generated MockRpc
    impl reth_btc_wallet::bitcoind::RpcApi for MockRpc {
        // Generic method we never need in the synchroniser tests,
        // but it used by others
        fn call<T>(
            &self,
            _cmd: &str,
            _args: &[serde_json::Value],
        ) -> Result<T, reth_btc_wallet::bitcoind::JsonRPCError>
        where
            T: for<'a> serde::de::Deserialize<'a>,
        {
            panic!("MockRpc::call is not expected to be invoked in these tests")
        }

        // The rest just forward to the mockall generated methods

        fn get_block_header(
            &self,
            hash: &BitcoinBlockHash,
        ) -> Result<BitcoinHeader, reth_btc_wallet::bitcoind::JsonRPCError> {
            self.get_block_header(hash)
        }

        fn get_block_count(&self) -> Result<u64, reth_btc_wallet::bitcoind::JsonRPCError> {
            self.get_block_count()
        }

        fn get_block_hash(
            &self,
            height: u64,
        ) -> Result<BitcoinBlockHash, reth_btc_wallet::bitcoind::JsonRPCError> {
            self.get_block_hash(height)
        }
    }

    // This one depends on call as well so we can't do it with mock macro

    impl reth_btc_wallet::bitcoind::RpcApiExt for MockRpc {
        async fn is_synced(&self) -> Result<bool, reth_btc_wallet::bitcoind::BitcoindError> {
            Ok(true)
        }

        async fn wait_until_synced(&self) {}
    }

    /// Small helper to make a fake header
    fn create_header(prev_hash: BitcoinBlockHash) -> BitcoinHeader {
        BitcoinHeader {
            version: Default::default(),
            prev_blockhash: prev_hash,
            merkle_root: TxMerkleNode::all_zeros(),
            time: Default::default(),
            bits: Default::default(),
            nonce: Default::default(),
        }
    }

    fn expect_header_chain(
        mock: &mut MockRpc,
        heights: std::ops::RangeInclusive<u64>,
        mut prev_hash: BitcoinBlockHash,
    ) {
        for height in heights {
            let header = create_header(prev_hash);
            let new_hash = header.block_hash();

            mock.expect_get_block_hash().with(eq(height)).returning(move |_| Ok(new_hash));

            mock.expect_get_block_header().with(eq(new_hash)).returning(move |_| Ok(header));

            prev_hash = new_hash;
        }
    }
}
