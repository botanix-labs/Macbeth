//! Module for providing authority metrics.
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};

/// Metrics for the entire network, handled by `NetworkManager`
#[derive(Clone, Metrics)]
#[metrics(scope = "authority")]
pub struct AuthorityMetrics {
    /// Number of currently connected peers
    pub(crate) signing_sessions: Gauge,

    /// Number of received round1 DKG packages
    pub(crate) received_round1_dkg_packages: Counter,

    /// Number of received round2 DKG packages
    pub(crate) received_round2_dkg_packages: Counter,

    /// Number of created agg pub keys
    pub(crate) created_agg_pub_keys: Counter,

    /// Number of received round1 signing packages
    pub(crate) received_round1_signing_packages: Counter,

    /// Number of received round2 signing packages
    pub(crate) received_round2_signing_packages: Counter,

    /// Number of finalized signings
    pub(crate) finalized_signings: Counter,

    #[allow(dead_code)]
    /// Number of reset wallet states
    pub(crate) reset_wallet_states: Counter,

    /// Number of commet finalized blocks
    pub(crate) commet_finalized_blocks: Counter,

    /// Number of commet committed blocks
    pub(crate) commet_committed_blocks: Counter,

    /// Number of commet prepared proposals
    pub(crate) commet_prepared_proposals: Counter,

    /// Number of commet processd\ed proposals
    pub(crate) commet_processed_proposals: Counter,
}

/// Measures the duration of executing the given code block. The duration is added to the given
/// accumulator value passed as a mutable reference.
#[macro_export]
macro_rules! duration_metered_exec {
    ($code:expr, $acc:expr) => {{
        let start = std::time::Instant::now();

        let res = $code;

        $acc += start.elapsed();

        res
    }};
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use metrics::Key;
    use metrics_util::{
        debugging::{DebugValue, DebuggingRecorder},
        CompositeKey, MetricKind,
    };

    #[test]
    #[allow(clippy::mutable_key_type)]
    fn test_authority_metrics_operations() {
        let recorder = DebuggingRecorder::new();

        metrics::with_local_recorder(&recorder, || {
            let metrics = AuthorityMetrics::default();

            AuthorityMetrics::describe();

            metrics.signing_sessions.set(5.0);
            metrics.received_round1_dkg_packages.increment(3);
            metrics.received_round2_dkg_packages.increment(4);
            metrics.created_agg_pub_keys.increment(2);
            metrics.received_round1_signing_packages.increment(7);
            metrics.received_round2_signing_packages.increment(6);
            metrics.finalized_signings.increment(1);
            metrics.reset_wallet_states.increment(2);
            metrics.commet_finalized_blocks.increment(3);
            metrics.commet_committed_blocks.increment(1);
            metrics.commet_prepared_proposals.increment(5);
            metrics.commet_processed_proposals.increment(4);

            let snapshots = recorder.snapshotter().snapshot();

            let metrics_map = snapshots.into_hashmap();

            // verify the gauge
            let gauge_key = Key::from_name("authority.signing_sessions");
            let gauge_value = metrics_map.get(&CompositeKey::new(MetricKind::Gauge, gauge_key));
            assert!(gauge_value.is_some(), "signing_sessions gauge not found");
            if let Some((_, _, DebugValue::Gauge(value))) = gauge_value {
                assert_eq!(*value, 5.0, "signing_sessions value incorrect");
            } else {
                panic!("signing_sessions has wrong type");
            }

            // verify all counters
            let counters_to_check = [
                ("authority.received_round1_dkg_packages", 3),
                ("authority.received_round2_dkg_packages", 4),
                ("authority.created_agg_pub_keys", 2),
                ("authority.received_round1_signing_packages", 7),
                ("authority.received_round2_signing_packages", 6),
                ("authority.finalized_signings", 1),
                ("authority.reset_wallet_states", 2),
                ("authority.commet_finalized_blocks", 3),
                ("authority.commet_committed_blocks", 1),
                ("authority.commet_prepared_proposals", 5),
                ("authority.commet_processed_proposals", 4),
            ];

            for (counter_name, expected_value) in counters_to_check {
                let counter_key = Key::from_name(counter_name);
                let counter_value =
                    metrics_map.get(&CompositeKey::new(MetricKind::Counter, counter_key));
                assert!(counter_value.is_some(), "{} counter not found", counter_name);
                if let Some((_, _, DebugValue::Counter(value))) = counter_value {
                    assert_eq!(*value, expected_value, "{} value incorrect", counter_name);
                } else {
                    panic!("{} has wrong type", counter_name);
                }
            }
        });
    }

    #[test]
    fn test_duration_metered_exec_basic() {
        let mut accumulated_duration = Duration::from_secs(0);

        let result = duration_metered_exec!(
            {
                std::thread::sleep(Duration::from_millis(10));
                42
            },
            accumulated_duration
        );

        assert_eq!(result, 42);

        assert!(
            accumulated_duration.as_millis() >= 10,
            "Duration should be at least 10ms, got {:?}",
            accumulated_duration
        );
    }

    #[test]
    fn test_duration_metered_exec_accumulation() {
        let mut accumulated_duration = Duration::from_secs(0);

        duration_metered_exec!(
            {
                std::thread::sleep(Duration::from_millis(10));
            },
            accumulated_duration
        );

        let first_measurement = accumulated_duration;
        assert!(
            first_measurement.as_millis() >= 10,
            "First measurement should be at least 10ms, got {:?}",
            first_measurement
        );

        duration_metered_exec!(
            {
                std::thread::sleep(Duration::from_millis(15));
            },
            accumulated_duration
        );

        assert!(
            accumulated_duration > first_measurement,
            "Duration should have increased, was {:?}, now {:?}",
            first_measurement,
            accumulated_duration
        );

        assert!(
            accumulated_duration.as_millis() >= 25,
            "Accumulated duration should be at least 25ms, got {:?}",
            accumulated_duration
        );
    }

    #[test]
    fn test_duration_metered_exec_zero_duration() {
        let mut accumulated_duration = Duration::from_secs(0);

        let result = duration_metered_exec!({ 5 + 5 }, accumulated_duration);

        assert_eq!(result, 10);
        assert!(
            accumulated_duration > Duration::from_nanos(0),
            "Should record some duration, got {:?}",
            accumulated_duration
        );
    }

    #[test]
    fn test_duration_metered_exec_with_existing_duration() {
        let mut accumulated_duration = Duration::from_millis(100);

        duration_metered_exec!(
            {
                std::thread::sleep(Duration::from_millis(10));
            },
            accumulated_duration
        );

        assert!(
            accumulated_duration.as_millis() >= 110,
            "Should preserve existing duration and add new measurement, got {:?}",
            accumulated_duration
        );
    }

    #[tokio::test]
    async fn test_duration_metered_exec_async() {
        let mut accumulated_duration = Duration::from_secs(0);

        let result = duration_metered_exec!(
            {
                async {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    "async result"
                }
                .await
            },
            accumulated_duration
        );

        assert_eq!(result, "async result");
        assert!(
            accumulated_duration.as_millis() >= 10,
            "Duration should be at least 10ms, got {:?}",
            accumulated_duration
        );
    }

    #[tokio::test]
    async fn test_duration_metered_exec_nested_async() {
        let mut accumulated_duration = Duration::from_secs(0);

        let result = duration_metered_exec!(
            {
                async {
                    let inner = async {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        42
                    }
                    .await;

                    inner * 2
                }
                .await
            },
            accumulated_duration
        );

        assert_eq!(result, 84);
        assert!(
            accumulated_duration.as_millis() >= 10,
            "Duration should be at least 10ms, got {:?}",
            accumulated_duration
        );
    }

    #[tokio::test]
    async fn test_duration_metered_exec_multiple_awaits() {
        let mut accumulated_duration = Duration::from_secs(0);

        let result = duration_metered_exec!(
            {
                async {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    let val1 = 40;

                    tokio::time::sleep(Duration::from_millis(15)).await;
                    let val2 = 2;

                    val1 + val2
                }
                .await
            },
            accumulated_duration
        );

        assert_eq!(result, 42);
        assert!(
            accumulated_duration.as_millis() >= 25,
            "Duration should be at least 25ms, got {:?}",
            accumulated_duration
        );
    }

    #[test]
    fn test_duration_metered_exec_error_handling() {
        let mut accumulated_duration = Duration::from_secs(0);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            duration_metered_exec!(
                {
                    if true {
                        panic!("Test panic");
                    }
                    42
                },
                accumulated_duration
            )
        }));

        assert!(result.is_err());

        assert_eq!(accumulated_duration, Duration::from_secs(0));
    }
}
