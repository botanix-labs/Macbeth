use bitcoin::hashes::Hash;
use bitcoin::BlockHash as BitcoinBlockHash;
use bitcoincore_zmq::{Message, SocketMessage};
use futures::Stream;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::time::Sleep;

/// A stream for testing or simulation that emits dummy Bitcoin block hash messages at regular time intervals.
///
/// This stream mimics a ZMQ block notification feed by periodically yielding
/// dummy `SocketMessage::Message(Message::HashBlock)` items with a placeholder (all zeros) block hash.
/// It can be used in tests or in scenarios where a real ZMQ connection is not available.
///
/// - The first message is emitted immediately upon the first poll.
/// - Each subsequent message is emitted after the configured `interval` elapses.
/// - Can be rapidly polled (even with a zero interval) for high-frequency message generation.
pub struct DummyHashBlockStream {
    /// The interval between generated messages.
    interval: Duration,
    /// Tracks if the stream has been polled for the first time.
    first_poll: bool,
    /// Sleep future for timing between messages.
    sleep: Option<Pin<Box<Sleep>>>,
}

impl DummyHashBlockStream {
    /// Constructs a new dummy stream that yields block hash messages every `interval`.
    ///
    /// # Arguments
    ///
    /// * `interval` — Minimum duration to wait between consecutive messages (except the first, which is immediate).
    pub fn new(interval: Duration) -> Self {
        Self { interval, first_poll: true, sleep: None }
    }
}

impl Stream for DummyHashBlockStream {
    type Item = Result<SocketMessage, bitcoincore_zmq::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.first_poll {
            // First poll returns immediately
            self.first_poll = false;
            return dummy_hash_block_message_ready();
        }

        // Check if we need to start sleeping
        if self.sleep.is_none() {
            self.sleep = Some(Box::pin(tokio::time::sleep(self.interval)));
        }

        // Poll the sleep future
        if let Some(sleep) = &mut self.sleep {
            match sleep.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    // Sleep completed, reset for next time and return message
                    self.sleep = None;
                    dummy_hash_block_message_ready()
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            // This shouldn't happen, but handle it gracefully
            dummy_hash_block_message_ready()
        }
    }
}

fn dummy_hash_block_message_ready() -> Poll<Option<Result<SocketMessage, bitcoincore_zmq::Error>>> {
    let dummy_hash = BitcoinBlockHash::all_zeros();
    Poll::Ready(Some(Ok(SocketMessage::Message(Message::HashBlock(dummy_hash, 0)))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::time::Duration;
    use tokio::time::Instant;

    #[tokio::test]
    async fn test_dummy_hash_block_stream_creation() {
        let interval = Duration::from_millis(100);
        let stream = DummyHashBlockStream::new(interval);

        // Verify that the stream was created with first_poll flag set to true
        assert!(stream.first_poll);
        assert_eq!(stream.interval, interval);
    }

    #[tokio::test]
    async fn test_first_poll_returns_immediately() {
        let stream = DummyHashBlockStream::new(Duration::from_secs(1));
        let mut stream = Box::pin(stream);

        let start = Instant::now();
        let result = stream.next().await;
        let elapsed = start.elapsed();

        // First poll should return immediately (within a reasonable time)
        assert!(elapsed < Duration::from_millis(100));
        assert!(result.is_some());

        if let Some(Ok(SocketMessage::Message(Message::HashBlock(hash, height)))) = result {
            assert_eq!(hash, BitcoinBlockHash::all_zeros());
            assert_eq!(height, 0);
        } else {
            panic!("Expected HashBlock message");
        }
    }

    #[tokio::test]
    async fn test_subsequent_polls_respect_interval() {
        let interval = Duration::from_millis(100);
        let stream = DummyHashBlockStream::new(interval);
        let mut stream = Box::pin(stream);

        // Get first message (should be immediate)
        let _first = stream.next().await;

        // Get second message (should wait for interval)
        let start = Instant::now();
        let result = stream.next().await;
        let elapsed = start.elapsed();

        // Should have waited approximately the interval duration
        // Allow for some tolerance due to timing variations
        assert!(elapsed >= Duration::from_millis(95)); // Allow 5ms tolerance below
        assert!(elapsed < Duration::from_millis(150)); // Allow 50ms tolerance above
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_stream_with_zero_interval() {
        let stream = DummyHashBlockStream::new(Duration::from_millis(0));
        let mut stream = Box::pin(stream);

        // Should be able to get messages rapidly
        let start = Instant::now();
        let _first = tokio::time::timeout(Duration::from_secs(1), stream.next()).await.unwrap();
        let _second = tokio::time::timeout(Duration::from_secs(1), stream.next()).await.unwrap();
        let elapsed = start.elapsed();

        // Both messages should arrive quickly (zero interval means immediate)
        assert!(elapsed < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn test_concurrent_polling() {
        let interval = Duration::from_millis(100);
        let stream =
            std::sync::Arc::new(tokio::sync::Mutex::new(DummyHashBlockStream::new(interval)));

        let mut handles = Vec::new();

        // Spawn multiple tasks that poll the stream with timeout
        for _ in 0..3 {
            let stream_clone = stream.clone();
            let handle = tokio::spawn(async move {
                let mut stream_guard = stream_clone.lock().await;
                let mut stream_pin = Pin::new(&mut *stream_guard);
                tokio::time::timeout(Duration::from_secs(2), stream_pin.next()).await
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete with timeout
        let results =
            tokio::time::timeout(Duration::from_secs(5), futures::future::join_all(handles))
                .await
                .expect("Tasks should complete within timeout");

        // At least one should succeed (the first poll)
        let successful_results: Vec<_> = results
            .into_iter()
            .filter_map(|r| r.ok())
            .filter_map(|r| r.ok())
            .filter(|r| r.is_some())
            .collect();

        assert!(!successful_results.is_empty());
    }

    #[tokio::test]
    async fn test_state_consistency() {
        let interval = Duration::from_millis(50);
        let stream = DummyHashBlockStream::new(interval);
        let mut stream = Box::pin(stream);

        // Get several messages and verify state consistency
        for i in 0..5 {
            let start = Instant::now();
            let result = tokio::time::timeout(Duration::from_secs(2), stream.next()).await;
            let elapsed = start.elapsed();

            assert!(result.is_ok(), "Message {} should not timeout", i);
            assert!(result.unwrap().is_some(), "Message {} should exist", i);

            if i == 0 {
                // First message should be immediate
                assert!(elapsed < Duration::from_millis(25));
            } else {
                // Subsequent messages should respect the interval
                // Allow some tolerance for timing variations
                assert!(elapsed >= Duration::from_millis(45)); // Allow 5ms tolerance
                assert!(elapsed < Duration::from_millis(75)); // Allow 25ms tolerance
            }
        }
    }

    #[tokio::test]
    async fn test_stream_timing_precision() {
        let interval = Duration::from_millis(100);
        let stream = DummyHashBlockStream::new(interval);
        let mut stream = Box::pin(stream);

        // Get first message (immediate)
        let _first = stream.next().await;

        // Time multiple subsequent messages to verify consistent timing
        let mut timings = Vec::new();
        for _ in 0..3 {
            let start = Instant::now();
            let _message = tokio::time::timeout(Duration::from_secs(1), stream.next())
                .await
                .expect("Should not timeout");
            timings.push(start.elapsed());
        }

        // All timings should be approximately the interval duration
        for (i, timing) in timings.iter().enumerate() {
            assert!(
                *timing >= Duration::from_millis(95) && *timing <= Duration::from_millis(150),
                "Timing {} ({:?}) should be close to interval duration",
                i,
                timing
            );
        }
    }
}
