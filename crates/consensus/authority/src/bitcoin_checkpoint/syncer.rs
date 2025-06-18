//! Bitcoin checkpoint synchronization.
//!
//! This module handles synchronization of Bitcoin checkpoints chain with the Bitcoin network
//! using a Bitcoin RPC connection.

use super::chain::BitcoinCheckpointsChain;
use super::checkpoint::BitcoinCheckpoint;
use super::error::BitcoinCheckpointError;
use bitcoin::block::BlockHash as BitcoinBlockHash;
use bitcoincore_zmq::{Message, SocketEvent, SocketMessage};
use futures::Stream;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex as TokioMutex, Mutex};

/// Bitcoin block hash stream
pub type BitcoinHashBlockStream =
    Box<dyn Stream<Item = Result<SocketMessage, bitcoincore_zmq::Error>> + Send + Unpin + 'static>;

/// A delay to avoid busy loops
const SAFE_DELAY: Duration = Duration::from_secs(1);

macro_rules! map_rpc_error {
    ($target:expr, $method:ident ( $($args:tt)* )) => {{
        $target.$method($($args)*).map_err(|error| {
            BitcoinCheckpointError::SyncRpcError {
                error,
                procedure_name: stringify!($method).to_string(),
            }
        })
    }};
}

/// Information about a synchronized Bitcoin checkpoint.
///
/// Contains only the essential information needed for reporting successful
/// synchronization of a checkpoint.
#[derive(Debug)]
struct SyncedCheckpointInfo {
    /// Block height of the checkpoint
    #[allow(dead_code)]
    height: u32,
    /// Block hash of the checkpoint
    #[allow(dead_code)]
    hash: BitcoinBlockHash,
}

/// Conversion from a [BitcoinCheckpoint] to [SyncedCheckpointInfo].
impl From<&BitcoinCheckpoint> for SyncedCheckpointInfo {
    fn from(checkpoint: &BitcoinCheckpoint) -> Self {
        Self { height: checkpoint.height, hash: checkpoint.hash }
    }
}

/// The [BitcoinCheckpointsChainSynchronizer::handle_new_blocks_sync_result] returns `SyncLoopControl` to signal
/// to the main sync loop do we need to start the sync process right away
/// or sleep before the next sync
#[derive(Debug, Clone, Copy)]
enum SyncLoopControl {
    /// Wait for a new block to arrive and then sync again
    WaitForNewBlock,
    /// Sync checkpoints right away
    Sync,
}

/// Synchronizes a Bitcoin checkpoints chain with the Bitcoin network.
///
/// This structure manages the synchronization process between a Bitcoin node (via RPC)
/// and our local checkpoints chain, ensuring the chain is kept up to date with
/// confirmed blocks from the Bitcoin network.
pub struct BitcoinCheckpointsChainSynchronizer<R> {
    /// RPC client for communicating with a Bitcoin node
    rpc: R,
    /// The checkpoints chain to be synchronized
    checkpoints_chain: Arc<BitcoinCheckpointsChain>,
    /// The height of the last Bitcoin block that has been processed
    /// RPC is using u64 for block height for some reason, so we use it as well to avoid casting
    last_synced_height: Option<u64>,
}

impl<R> BitcoinCheckpointsChainSynchronizer<R>
where
    R: reth_btc_wallet::bitcoind::RpcApiExt,
{
    /// Creates a new Bitcoin checkpoints chain synchronizer.
    ///
    /// # Arguments
    ///
    /// * `checkpoints_chain` - The chain to synchronize with Bitcoin network
    /// * `rpc` - The Bitcoin RPC client used to fetch blockchain data
    ///
    /// # Returns
    ///
    /// A new synchronizer instance with the last synced height initialized from the chain.
    pub fn new(checkpoints_chain: Arc<BitcoinCheckpointsChain>, rpc: R) -> Self {
        // Calculate the last synced height based on the most recent checkpoint
        // in chain and the lowest confirmation depth
        let last_synced_height = checkpoints_chain
            .recent_height()
            .map(|height| height as u64 + checkpoints_chain.lowest_confirmation_depth() as u64);

        Self { rpc, checkpoints_chain, last_synced_height }
    }

    /// Synchronizes new Bitcoin blocks to the checkpoints chain.
    ///
    /// This method:
    /// 1. Fetches the current Bitcoin blockchain height
    /// 2. Determines which blocks need to be synced based on:
    ///    - The last height we've already synchronized
    ///    - The minimum required confirmation depth
    ///    - The maximum size limit of the checkpoints chain
    /// 3. Fetches block headers for the required blocks
    /// 4. Adds them to the checkpoints chain
    ///
    /// # Returns
    ///
    /// A vector of information about all successfully synchronized checkpoints,
    /// or an error if synchronization failed.
    ///
    /// # Errors
    ///
    /// It will return [BitcoinCheckpointError::StaleBlockAdded] error if a new block arrives during sync.
    fn sync_new_blocks(&mut self) -> Result<Vec<SyncedCheckpointInfo>, BitcoinCheckpointError> {
        let tip_height = map_rpc_error!(self.rpc, get_block_count())?;

        let last_synced_height = self.last_synced_height.unwrap_or_default();

        // Don't sync if we're at the same height
        if tip_height <= last_synced_height {
            tracing::debug!(last_synced_height, tip_height, "No new blocks to sync");
            return Ok(Vec::new());
        }

        // Is the chain too young to have any blocks with the
        // required lowest confirmation depth?
        let lowest_confirmation_depth = self.checkpoints_chain.lowest_confirmation_depth() as u64;
        if tip_height < lowest_confirmation_depth {
            // Not enough blocks yet.
            // Remember the new tip and return.
            self.last_synced_height = Some(tip_height);

            tracing::debug!(
                last_synced_height,
                lowest_confirmation_depth,
                tip_height,
                "Not enough blocks to sync checkpoints"
            );

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

            tracing::debug!(
                last_synced_height,
                lowest_confirmation_depth,
                tip_height,
                confirmed_available,
                confirmed_already,
                need_to_sync,
                "No new blocks to sync, already synced all available checkpoints"
            );

            return Ok(Vec::new());
        }

        // How many blocks we need to sync (limited by the chain size)
        let chain_size_limit = self.checkpoints_chain.size_limit() as u64;
        let blocks_to_sync = need_to_sync.min(chain_size_limit);

        // Use saturating subtraction to prevent overflow when tip_height is small
        let top_confirmed_height = tip_height.saturating_sub(lowest_confirmation_depth - 1);

        // We push from oldest to newest, so we start `blocks_to_sync`−1 below the top.
        // Use saturating subtraction to prevent overflow when blocks_to_sync > top_confirmed_height
        let from_height = top_confirmed_height.saturating_sub(blocks_to_sync - 1);
        let to_height = top_confirmed_height;

        let mut synced_checkpoints = Vec::new();
        for height in from_height..=to_height {
            let confirmed_hash = map_rpc_error!(self.rpc, get_block_hash(height))?;
            let header = map_rpc_error!(self.rpc, get_block_header(&confirmed_hash))?;

            // Create, report and push the checkpoint
            let bitcoin_checkpoint = BitcoinCheckpoint::new(header, height as u32);

            synced_checkpoints.push(SyncedCheckpointInfo::from(&bitcoin_checkpoint));

            tracing::trace!(
                ?bitcoin_checkpoint,
                "Add new bitcoin checkpoint for height {} to the checkpoints chain",
                height,
            );

            self.checkpoints_chain.push(bitcoin_checkpoint)?;
        }

        // Update the last height we've seen
        self.last_synced_height = Some(tip_height);

        Ok(synced_checkpoints)
    }

    /// Runs the checkpoint chain synchronizer in a continuous loop, monitoring the Bitcoin chain and updating checkpoints as new blocks arrive.
    ///
    /// # Arguments
    ///
    /// * `bitcoin_block_hash_stream` — A stream of messages related to Bitcoin block hashes. This stream can originate from a real ZMQ source (such as a Bitcoin Core node) or a test/dummy source emitting simulated block hash notifications.
    ///
    /// # Behavior
    ///
    /// This method orchestrates the following workflow in an infinite loop:
    /// 1. Listens for events from the provided block hash stream to detect when new Bitcoin blocks are mined.
    /// 2. On each trigger (block event or startup), spawns a background task to:
    ///     - Query the current Bitcoin tip via the provided RPC interface
    ///     - Determine which blocks (if any) require new checkpoints, based on configured confirmation rules
    ///     - Fetch block headers and append them to the checkpoint chain
    ///     - Handle any RPC errors, retrying after a delay as needed
    /// 3. Ensures at most one sync is running at once, throttling excessive triggers until in-flight sync completes
    /// 4. Sleeps for a short, configured delay between sync cycles to avoid excessive polling
    ///
    /// All relevant errors (such as RPC failures) are logged and automatically retried as part of the loop.
    /// This method is designed to run forever; it returns only if the stream ends or a fatal error is encountered.
    ///
    /// # Notes
    ///
    /// - Intended to be run as a background task (e.g., via `tokio::spawn`), since it never returns.
    /// - Uses interior mutability to safely share state between async tasks that may signal or run synchronizations.
    /// - Synces at startup to ensure no missed blocks prior to beginning event stream consumption.
    pub async fn sync(self, bitcoin_block_hash_stream: BitcoinHashBlockStream) {
        // We need interior mutability so that we can move `self` into spawn_blocking
        let syncer_lock = Arc::new(TokioMutex::new(self));

        // Create a channel to signal when we need to sync checkpoints
        // We use a bounded channel for throttling: if sync is already in progress,
        // we don't trigger sync until the previous one is processed.
        let (tx, mut rx) = mpsc::channel::<()>(1);

        // Spawn a task to consume the ZMQ stream and signal sync needs
        tokio::spawn(handle_hash_block_stream_messages(bitcoin_block_hash_stream, tx.clone()));

        // Sync at the start to ensure we have the latest checkpoints
        tracing::debug!("Syncing bitcoin checkpoints at the start");
        trigger_checkpoints_sync(tx.clone());

        while rx.recv().await.is_some() {
            let syncer_lock_clone = Arc::clone(&syncer_lock);

            let result = tokio::task::spawn_blocking(move || {
                let mut syncer = syncer_lock_clone.blocking_lock();
                syncer.sync_new_blocks()
            })
            .await
            .expect("spawned blocking task failed to sync bitcoin checkpoints");

            match handle_new_blocks_sync_result(result, Arc::clone(&syncer_lock)).await {
                SyncLoopControl::WaitForNewBlock => {
                    tracing::trace!("Waiting for new block to sync checkpoints");
                }
                SyncLoopControl::Sync => {
                    tracing::trace!("Immediately syncing checkpoints requested");

                    trigger_checkpoints_sync(tx.clone())
                }
            };

            tokio::time::sleep(SAFE_DELAY).await;
        }
    }
}

fn trigger_checkpoints_sync(tx: mpsc::Sender<()>) {
    match tx.try_send(()) {
        Ok(_) => {
            // If we successfully sent a message, we can proceed to sync
            tracing::trace!("Trigger checkpoint sync task");
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
            // If the channel is full, we skip this message
            tracing::trace!("Sync task is busy, skipping new block hash message");
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            // If the channel is closed, we stop processing messages
            panic!("Checkpoints sync task channel is closed, stopping processing bitcoin block hash messages");
        }
    };
}

async fn handle_hash_block_stream_messages<S, E>(
    mut bitcoin_block_hash_stream: S,
    tx: mpsc::Sender<()>,
) where
    S: Stream<Item = Result<SocketMessage, E>> + Unpin,
    E: std::fmt::Debug + std::fmt::Display,
{
    loop {
        while let Some(msg) = bitcoin_block_hash_stream.next().await {
            match msg {
                Ok(SocketMessage::Message(Message::HashBlock(bitcoin_block_hash, _))) => {
                    tracing::trace!(
                        %bitcoin_block_hash,
                        "Received new bitcoin block hash message"
                    );

                    trigger_checkpoints_sync(tx.clone());
                }
                Ok(SocketMessage::Message(message)) => {
                    tracing::warn!(
                        ?message,
                        "Received unexpected message from bitcoin block hash stream: {message}"
                    );
                }
                Ok(SocketMessage::Event(message)) => {
                    tracing::trace!(?message, "Received socket event {:?}", message.event);

                    match message.event {
                        SocketEvent::Disconnected { .. } => {
                            tracing::warn!(
                                "Disconnected from {}, ZMQ automatically tries to reconnect",
                                message.source_url
                            );
                        }
                        SocketEvent::HandshakeSucceeded => {
                            // We can say "reconnected" because subscribe_async_wait_handshake waits on
                            // the first connections of each endpoint before returning.
                            tracing::info!(
                                "Successfully reconnected to bitcoin zmq hash block socket {}",
                                message.source_url
                            );
                        }
                        _ => {
                            // ignore other events
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        "Error receiving message from bitcoin block hash stream: {err}"
                    );
                }
            }
        }

        tracing::warn!(
            "Bitcoin block hash stream ended, starting again in {} seconds",
            SAFE_DELAY.as_secs()
        );

        tokio::time::sleep(SAFE_DELAY).await
    }
}

async fn handle_new_blocks_sync_result<R>(
    result: Result<Vec<SyncedCheckpointInfo>, BitcoinCheckpointError>,
    syncer_lock: Arc<Mutex<BitcoinCheckpointsChainSynchronizer<R>>>,
) -> SyncLoopControl {
    match result {
        Ok(synced_checkpoints) => {
            tracing::info!(
                ?synced_checkpoints,
                "Async task synced {} bitcoin checkpoints",
                synced_checkpoints.len()
            );

            // We need to sleep before the next sync
            SyncLoopControl::WaitForNewBlock
        }
        Err(BitcoinCheckpointError::StaleBlockAdded {
            expected_prev_block_hash,
            received_prev_block_hash,
        }) => {
            // if we are getting a stale block, which is not corresponding to the existing checkpoint chain
            // let's clean up the chain and start syncing from scratch.
            let mut syncer = syncer_lock.lock().await;
            syncer.checkpoints_chain.clear();
            syncer.last_synced_height = None;

            tracing::warn!(
                %expected_prev_block_hash,
                %received_prev_block_hash,
                "Async task failed to add a stale block to the checkpoint chain due hashes mismatch. Reset the chain and start syncing immediately.",
            );

            // We reset checkpoints, so we need to sync them ASAP
            SyncLoopControl::Sync
        }
        Err(e) => {
            tracing::warn!("Async task failed to sync bitcoin checkpoints: {e}");

            // We need to try again
            SyncLoopControl::Sync
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
                matches!(result, Err(BitcoinCheckpointError::SyncRpcError { procedure_name, .. }) if procedure_name == "get_block_count")
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

    mod handle_new_blocks_sync_result {
        use super::*;

        #[tokio::test]
        async fn test_stale_block_resets_chain_and_height_and_requests_sync() {
            // Configure a chain with some data
            let chain =
                Arc::new(BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create valid chain"));

            let initial_header = create_header(BitcoinBlockHash::all_zeros());
            let initial_checkpoint = BitcoinCheckpoint::new(initial_header, 1);
            chain.push(initial_checkpoint).expect("push initial checkpoint");

            let mock = MockRpc::new();

            // Create our synchronizer with initial state
            let syncer = BitcoinCheckpointsChainSynchronizer::new(Arc::clone(&chain), mock);

            // Should be initialized from chain
            assert_eq!(syncer.last_synced_height, Some(5));

            let syncer_lock = Arc::new(Mutex::new(syncer));

            let result = Err(BitcoinCheckpointError::StaleBlockAdded {
                expected_prev_block_hash: BitcoinBlockHash::all_zeros(),
                received_prev_block_hash: BitcoinBlockHash::all_zeros(),
            });

            let control = handle_new_blocks_sync_result(result, Arc::clone(&syncer_lock)).await;

            assert!(matches!(control, SyncLoopControl::Sync));

            // Verify the chain was cleared and last_synced_height was reset
            let syncer = syncer_lock.lock().await;
            assert_eq!(syncer.last_synced_height, None);
            assert_eq!(syncer.checkpoints_chain.len(), 0);
        }

        #[tokio::test]
        async fn test_successful_sync_requests_wait_for_new_block() {
            // Configure a chain
            let chain =
                Arc::new(BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create valid chain"));
            let mock = MockRpc::new();
            let syncer = BitcoinCheckpointsChainSynchronizer::new(Arc::clone(&chain), mock);
            let syncer_lock = Arc::new(Mutex::new(syncer));

            // Create successful result with two checkpoints
            let h1 = BitcoinBlockHash::all_zeros();
            let h2 = BitcoinBlockHash::from_byte_array([2u8; 32]);
            let checkpoint1 = BitcoinCheckpoint::new(create_header(h1), 100);
            let checkpoint2 = BitcoinCheckpoint::new(create_header(h2), 101);

            let checkpoints = vec![
                SyncedCheckpointInfo::from(&checkpoint1),
                SyncedCheckpointInfo::from(&checkpoint2),
            ];

            let result = Ok(checkpoints);
            let control = handle_new_blocks_sync_result(result, Arc::clone(&syncer_lock)).await;

            // Verify we get WaitForNewBlock
            assert!(matches!(control, SyncLoopControl::WaitForNewBlock));
        }

        #[tokio::test]
        async fn test_generic_error_requests_sync() {
            // Configure a chain
            let chain =
                Arc::new(BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create valid chain"));
            let mock = MockRpc::new();
            let syncer = BitcoinCheckpointsChainSynchronizer::new(Arc::clone(&chain), mock);
            let syncer_lock = Arc::new(Mutex::new(syncer));

            // Create an RPC error
            let result = Err(BitcoinCheckpointError::SyncRpcError {
                error: reth_btc_wallet::bitcoind::JsonRPCError::UnexpectedStructure,
                procedure_name: "get_block_count".to_string(),
            });

            let control = handle_new_blocks_sync_result(result, Arc::clone(&syncer_lock)).await;

            // Verify we get Sync for generic errors
            assert!(matches!(control, SyncLoopControl::Sync));
        }
    }

    mod trigger_checkpoints_sync_tests {
        use super::*;
        use tokio::sync::mpsc;

        #[test]
        fn test_trigger_sync_successful() {
            let (tx, mut rx) = mpsc::channel::<()>(1);

            trigger_checkpoints_sync(tx);

            // Channel should have the message
            let try_recv = rx.try_recv();
            assert!(try_recv.is_ok());
        }

        #[test]
        fn test_trigger_sync_channel_full() {
            let (tx, _rx) = mpsc::channel::<()>(1);

            // Fill the channel
            let _ = tx.try_send(());

            // This should not panic even with a full channel
            trigger_checkpoints_sync(tx.clone());
        }

        #[test]
        fn test_trigger_sync_channel_closed() {
            let (tx, rx) = mpsc::channel::<()>(1);

            // Close the channel
            drop(rx);

            // This should panic, but we'll catch it
            let result = std::panic::catch_unwind(|| {
                trigger_checkpoints_sync(tx);
            });

            assert!(result.is_err());
        }
    }

    mod handle_hash_block_stream_messages {
        use super::*;
        use bitcoin::Txid;
        use bitcoincore_zmq::MonitorMessage;
        use std::pin::Pin;
        use std::task::{Context, Poll};
        use tokio::sync::mpsc;

        /// Mock MessageStream for testing
        struct MockMessageStream {
            messages: Vec<Result<SocketMessage, bitcoincore_zmq::Error>>,
        }

        impl MockMessageStream {
            fn new(messages: Vec<Result<SocketMessage, bitcoincore_zmq::Error>>) -> Self {
                Self { messages }
            }
        }

        impl Stream for MockMessageStream {
            type Item = Result<SocketMessage, bitcoincore_zmq::Error>;

            fn poll_next(
                mut self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<Option<Self::Item>> {
                if self.messages.is_empty() {
                    return Poll::Ready(None);
                }
                Poll::Ready(Some(self.messages.remove(0)))
            }
        }

        /// Test helper to run the handler and collect sent messages
        async fn run_handler_with_messages(
            messages: Vec<Result<SocketMessage, bitcoincore_zmq::Error>>,
        ) -> Vec<()> {
            let (tx, mut rx) = mpsc::channel::<()>(10);
            let stream = MockMessageStream::new(messages);

            // Spawn handler with timeout to ensure it doesn't run forever
            let handler = tokio::spawn(async move {
                tokio::select! {
                    _ = handle_hash_block_stream_messages(stream, tx) => {},
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {},
                }
            });

            // Collect all messages sent on the channel
            let mut received = Vec::new();
            while let Ok(Some(msg)) =
                tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
            {
                received.push(msg);
            }

            // Make sure handler is done
            let _ = handler.await;

            received
        }

        #[tokio::test]
        async fn test_handle_hash_block_message() {
            let hash = BitcoinBlockHash::from_byte_array([42u8; 32]);

            let messages = vec![Ok(SocketMessage::Message(Message::HashBlock(hash, 0)))];

            let received = run_handler_with_messages(messages).await;

            // Should trigger one sync
            assert_eq!(received.len(), 1);
        }

        #[tokio::test]
        async fn test_handle_unexpected_message() {
            let txid = Txid::from_byte_array([1u8; 32]);

            // Create an unexpected message type
            let messages = vec![Ok(SocketMessage::Message(Message::HashTx(txid, 0)))];

            let received = run_handler_with_messages(messages).await;

            // Should not trigger a sync for non-HashBlock messages
            assert_eq!(received.len(), 0);
        }

        #[tokio::test]
        async fn test_handle_disconnected_event() {
            // Create a disconnected event
            let message = MonitorMessage {
                event: SocketEvent::from_raw(512, 1).unwrap(),
                source_url: "tcp://localhost:1234".to_string(),
            };

            let messages = vec![Ok(SocketMessage::Event(message))];

            let received = run_handler_with_messages(messages).await;

            // Should not trigger a sync for disconnected event
            assert_eq!(received.len(), 0);
        }

        #[tokio::test]
        async fn test_handle_handshake_succeeded_event() {
            // Create a handshake succeeded event
            let message = MonitorMessage {
                event: SocketEvent::HandshakeSucceeded,
                source_url: "tcp://localhost:1234".to_string(),
            };

            let messages = vec![Ok(SocketMessage::Event(message))];

            let received = run_handler_with_messages(messages).await;

            // Should not trigger a sync for handshake event
            assert_eq!(received.len(), 0);
        }

        #[tokio::test]
        async fn test_handle_stream_error() {
            // Create a stream error
            let messages = vec![Err(bitcoincore_zmq::Error::Invalid256BitHashLength(0))];

            let received = run_handler_with_messages(messages).await;

            // Should not trigger a sync for stream error
            assert_eq!(received.len(), 0);
        }

        #[tokio::test]
        async fn test_handle_multiple_hash_block_messages() {
            let hash1 = BitcoinBlockHash::from_byte_array([1u8; 32]);
            let hash2 = BitcoinBlockHash::from_byte_array([2u8; 32]);

            let messages = vec![
                Ok(SocketMessage::Message(Message::HashBlock(hash1, 0))),
                Ok(SocketMessage::Message(Message::HashBlock(hash2, 0))),
            ];

            let received = run_handler_with_messages(messages).await;

            // Should trigger a sync for each HashBlock message
            assert_eq!(received.len(), 2);
        }

        #[tokio::test]
        async fn test_handle_mixed_messages() {
            let hash = BitcoinBlockHash::from_byte_array([42u8; 32]);

            let txid = Txid::from_byte_array([1u8; 32]);

            // Create a mix of message types
            let event = MonitorMessage {
                event: SocketEvent::HandshakeSucceeded,
                source_url: "tcp://localhost:1234".to_string(),
            };

            let messages = vec![
                Ok(SocketMessage::Event(event)),
                Ok(SocketMessage::Message(Message::HashTx(txid, 0))),
                Ok(SocketMessage::Message(Message::HashBlock(hash, 0))),
                Err(bitcoincore_zmq::Error::Invalid256BitHashLength(0)),
            ];

            let received = run_handler_with_messages(messages).await;

            // Should only trigger a sync for the HashBlock message
            assert_eq!(received.len(), 1);
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
