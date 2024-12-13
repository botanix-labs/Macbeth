use prometheus::{
    register_histogram_vec, register_int_counter_vec, register_int_gauge_vec, HistogramVec,
    IntCounterVec, IntGaugeVec, Registry,
};

#[macro_export]
macro_rules! update_telemetry_error {
    ($telemetry:expr, $error:expr) => {
        if let Some(telemetry) = $telemetry {
            telemetry.update_pegout_scheduler_error_metrics(&$error.to_string());
        }
    };
}
#[derive(Clone, Debug)]
pub struct BtcServerMetrics {
    pub registry: Registry,
    // signing
    pub total_signing_sessions: IntGaugeVec,
    pub total_aborted_signing_sessions: IntGaugeVec,
    pub total_finalized_signing_sessions: IntGaugeVec,
    pub total_received_round1_signing_packages: IntCounterVec,
    pub total_received_round2_signing_packages: IntCounterVec,
    pub total_failed_messages: IntCounterVec,
    pub round1_signing_throughput: IntCounterVec,
    pub round2_signing_throughput: IntCounterVec,
    pub round1_signing_latency_histogram: HistogramVec,
    pub round2_signing_latency_histogram: HistogramVec,
    pub round1_signing_package_size_histogram: HistogramVec,
    pub round2_signing_package_size_histogram: HistogramVec,
    pub signing_error_rates: IntCounterVec,
    //dkg
    pub total_received_round1_dkg_packages: IntCounterVec,
    pub total_received_round2_dkg_packages: IntCounterVec,
    pub round1_dkg_throughput: IntCounterVec,
    pub round2_dkg_throughput: IntCounterVec,
    pub round1_dkg_latency_histogram: HistogramVec,
    pub round2_dkg_latency_histogram: HistogramVec,
    pub round1_dkg_package_size_histogram: HistogramVec,
    pub round2_dkg_package_size_histogram: HistogramVec,
    pub dkg_error_rates: IntCounterVec,
    // pegout scheduler
    pub pegout_scheduler_error_rates: IntCounterVec,
    pub pending_pegouts: IntGaugeVec,
}

impl Default for BtcServerMetrics {
    fn default() -> Self {
        BtcServerMetrics::new(None).expect("Failed to create default BtcServerMetrics")
    }
}

impl BtcServerMetrics {
    pub fn new(prefix: Option<String>) -> anyhow::Result<Self> {
        let metric_prefix = prefix.clone().map(|p| format!("{}_", p)).unwrap_or_default();

        // ================================== signing ==================================
        let total_signing_sessions = register_int_gauge_vec!(
            format!("{}total_signing_sessions", metric_prefix),
            "A metric counting the number of total signing sessions",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let total_aborted_signing_sessions = register_int_gauge_vec!(
            format!("{}total_aborted_signing_sessions", metric_prefix),
            "A metric counting the number of total aborted signing sessions",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let total_finalized_signing_sessions = register_int_gauge_vec!(
            format!("{}total_finalized_signing_sessions", metric_prefix),
            "A metric counting the number of total finalized signing sessions",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let total_received_round1_signing_packages = register_int_counter_vec!(
            format!("{}total_received_round1_signing_packages", metric_prefix),
            "A metric counting the number of received round 1 packages",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let total_received_round2_signing_packages = register_int_counter_vec!(
            format!("{}total_received_round2_signing_packages", metric_prefix),
            "A metric counting the number of received round 2 packages",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let total_failed_messages = register_int_counter_vec!(
            format!("{}total_failed_messages", metric_prefix),
            "A metric counting the number of unpublished and failed messages",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let round1_signing_throughput = register_int_counter_vec!(
            format!("{}round1_signing_throughput", metric_prefix),
            "A metric counting the number of gossiped round1 signing messages per signing round and id",
            &["btc_chain", "self_id", "signing_session_id"],
        )
        .expect("metric must be created");

        let round2_signing_throughput = register_int_counter_vec!(
            format!("{}round2_signing_throughput", metric_prefix),
            "A metric counting the number of gossiped round2 signing messages per signing round and id",
            &["btc_chain", "self_id", "signing_session_id"],
        )
        .expect("metric must be created");

        // New histogram metric for block latency
        let round1_signing_latency_histogram = register_histogram_vec!(
            format!("{}round1_signing_latency_secs", metric_prefix),
            "Histogram of latencies between receiving and writing round1 signing package to db",
            &["btc_chain", "self_id"],
            // buckets for latency measurement (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0],
        )
        .expect("metric must be created");

        let round2_signing_latency_histogram = register_histogram_vec!(
            format!("{}round2_signing_latency_secs", metric_prefix),
            "Histogram of latencies between receiving and writing round2 signing package to db",
            &["btc_chain", "self_id"],
            // buckets for latency measurement (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0],
        )
        .expect("metric must be created");

        let round1_signing_package_size_histogram = register_histogram_vec!(
            format!("{}round1_signing_package_size_bytes", metric_prefix),
            "Histogram of round1 signing packages sizes in bytes",
            &["btc_chain", "self_id"],
            vec![100.0, 500.0, 1000.0, 5000.0, 10000.0, 100000.0, 1000000.0]
        )
        .expect("metric must be created");

        let round2_signing_package_size_histogram = register_histogram_vec!(
            format!("{}round2_signing_package_size_bytes", metric_prefix),
            "Histogram of round2 signing packages sizes in bytes",
            &["btc_chain", "self_id"],
            vec![100.0, 500.0, 1000.0, 5000.0, 10000.0, 100000.0, 1000000.0]
        )
        .expect("metric must be created");

        let signing_error_rates = register_int_counter_vec!(
            format!("{}signing_error_rates", metric_prefix),
            "A metric counting errors or failures during signing message processing",
            &["btc_chain", "self_id", "signing_session_id", "error_type"],
        )
        .expect("metric must be created");

        //  ================================== dkg ==================================
        let total_received_round1_dkg_packages = register_int_counter_vec!(
            format!("{}total_received_round1_dkg_packages", metric_prefix),
            "A metric counting the number of received round 1 dkg packages",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let total_received_round2_dkg_packages = register_int_counter_vec!(
            format!("{}total_received_round2_dkg_packages", metric_prefix),
            "A metric counting the number of received round 2 dkg packages",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let round1_dkg_throughput = register_int_counter_vec!(
            format!("{}round1_dkg_throughput", metric_prefix),
            "A metric counting the number of gossiped round1 dkg messages per id",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let round2_dkg_throughput = register_int_counter_vec!(
            format!("{}round2_dkg_throughput", metric_prefix),
            "A metric counting the number of gossiped round2 dkg messages per id",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        // New histogram metric for package latency
        let round1_dkg_latency_histogram = register_histogram_vec!(
            format!("{}round1_dkg_latency_secs", metric_prefix),
            "Histogram of latencies between receiving and writing dkg package to db",
            &["btc_chain", "self_id"],
            // buckets for latency measurement (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0],
        )
        .expect("metric must be created");

        let round2_dkg_latency_histogram = register_histogram_vec!(
            format!("{}round2_dkg_latency_secs", metric_prefix),
            "Histogram of latencies between receiving and writing round2 dkg package to db",
            &["btc_chain", "self_id"],
            // buckets for latency measurement (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0],
        )
        .expect("metric must be created");

        let round1_dkg_package_size_histogram = register_histogram_vec!(
            format!("{}round1_dkg_package_size_bytes", metric_prefix),
            "Histogram of round1 dkg packages sizes in bytes",
            &["btc_chain", "self_id"],
            vec![100.0, 500.0, 1000.0, 5000.0, 10000.0, 100000.0, 1000000.0]
        )
        .expect("metric must be created");

        let round2_dkg_package_size_histogram = register_histogram_vec!(
            format!("{}round2_dkg_package_size_bytes", metric_prefix),
            "Histogram of round2 dkg packages sizes in bytes",
            &["btc_chain", "self_id"],
            vec![100.0, 500.0, 1000.0, 5000.0, 10000.0, 100000.0, 1000000.0]
        )
        .expect("metric must be created");

        let dkg_error_rates = register_int_counter_vec!(
            format!("{}dkg_error_rates", metric_prefix),
            "A metric counting errors or failures during dkg message processing",
            &["btc_chain", "self_id", "error_type"],
        )
        .expect("metric must be created");

        let pegout_scheduler_error_rates = register_int_counter_vec!(
            format!("{}pegout_scheduler_error_rates", metric_prefix),
            "A metric counting errors or failures during the pegout scheduler processing",
            &[],
        )
        .expect("metric must be created");

        let pending_pegouts = register_int_gauge_vec!(
            format!("{}pending_pegouts", metric_prefix),
            "A metric counting the number of pending pegouts",
            &[],
        )
        .expect("metric must be created");

        // ====================================================================
        let registry = Registry::new_custom(prefix, None).expect("registry to be created");
        // signing
        registry.register(Box::new(total_signing_sessions.clone()))?;
        registry.register(Box::new(total_aborted_signing_sessions.clone()))?;
        registry.register(Box::new(total_finalized_signing_sessions.clone()))?;
        registry.register(Box::new(total_received_round1_signing_packages.clone()))?;
        registry.register(Box::new(total_received_round2_signing_packages.clone()))?;
        registry.register(Box::new(total_failed_messages.clone()))?;
        registry.register(Box::new(round1_signing_throughput.clone()))?;
        registry.register(Box::new(round2_signing_throughput.clone()))?;
        registry.register(Box::new(round1_signing_latency_histogram.clone()))?;
        registry.register(Box::new(round2_signing_latency_histogram.clone()))?;
        registry.register(Box::new(round1_signing_package_size_histogram.clone()))?;
        registry.register(Box::new(round2_signing_package_size_histogram.clone()))?;
        registry.register(Box::new(signing_error_rates.clone()))?;

        // dkg
        registry.register(Box::new(total_received_round1_dkg_packages.clone()))?;
        registry.register(Box::new(total_received_round2_dkg_packages.clone()))?;
        registry.register(Box::new(round1_dkg_throughput.clone()))?;
        registry.register(Box::new(round2_dkg_throughput.clone()))?;
        registry.register(Box::new(round1_dkg_latency_histogram.clone()))?;
        registry.register(Box::new(round2_dkg_latency_histogram.clone()))?;
        registry.register(Box::new(round1_dkg_package_size_histogram.clone()))?;
        registry.register(Box::new(round2_dkg_package_size_histogram.clone()))?;
        registry.register(Box::new(dkg_error_rates.clone()))?;

        // pegouts
        registry.register(Box::new(pegout_scheduler_error_rates.clone()))?;
        registry.register(Box::new(pending_pegouts.clone()))?;

        Ok(Self {
            registry,
            total_signing_sessions,
            total_aborted_signing_sessions,
            total_finalized_signing_sessions,
            total_received_round1_signing_packages,
            total_received_round2_signing_packages,
            total_failed_messages,
            round1_signing_throughput,
            round2_signing_throughput,
            round1_signing_latency_histogram,
            round2_signing_latency_histogram,
            round1_signing_package_size_histogram,
            round2_signing_package_size_histogram,
            signing_error_rates,

            total_received_round1_dkg_packages,
            total_received_round2_dkg_packages,
            round1_dkg_throughput,
            round2_dkg_throughput,
            round1_dkg_latency_histogram,
            round2_dkg_latency_histogram,
            round1_dkg_package_size_histogram,
            round2_dkg_package_size_histogram,
            dkg_error_rates,

            pegout_scheduler_error_rates,
            pending_pegouts,
        })
    }
}

#[cfg(test)]
mod tests {
    use prometheus::{gather, Encoder, TextEncoder};

    use super::*;

    impl BtcServerMetrics {
        pub fn random() -> Self {
            use rand::{distributions::Alphanumeric, Rng};

            let prefix = rand::thread_rng()
                .sample_iter(&Alphanumeric)
                .filter(|c| c.is_ascii_alphabetic())
                .take(6)
                .map(char::from)
                .collect();

            BtcServerMetrics::new(Some(prefix)).expect("Failed to create random BtcServerMetrics")
        }
    }

    #[test]
    fn test_round1_dkg_throughout_metric() {
        let metrics = BtcServerMetrics::random();

        metrics.round1_dkg_throughput.with_label_values(&["regtest", "0"]).inc_by(5);

        let metric_families = gather();
        let mut buffer = Vec::new();
        let encoder = TextEncoder::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();

        let output = String::from_utf8(buffer.clone()).unwrap();

        assert!(output.contains("round1_dkg_throughput"));
        assert!(output.contains("regtest"));
        assert!(output.contains("0"));
    }

    #[test]
    fn test_round1_dkg_latency_histogram_metric() {
        let metrics = BtcServerMetrics::random();

        metrics.round1_dkg_latency_histogram.with_label_values(&["regtest", "0"]).observe(0.75);

        let metric_families = gather();
        let mut buffer = Vec::new();
        let encoder = TextEncoder::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();

        let output = String::from_utf8(buffer.clone()).unwrap();

        assert!(output.contains("round1_dkg_latency_secs"));
        assert!(output.contains("regtest"));
        assert!(output.contains("0"));
    }

    #[test]
    fn test_round1_dkg_package_size_histogram_metric() {
        let metrics = BtcServerMetrics::random();

        metrics
            .round1_dkg_package_size_histogram
            .with_label_values(&["regtest", "1"])
            .observe(1500.1);

        let metric_families = gather();
        let mut buffer = Vec::new();
        let encoder = TextEncoder::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();

        let output = String::from_utf8(buffer.clone()).unwrap();

        assert!(output.contains("round1_dkg_package_size_bytes"));
        assert!(output.contains("regtest"));
        assert!(output.contains("1"));
    }

    #[test]
    fn test_total_failed_messages_metric() {
        let metrics = BtcServerMetrics::random();

        metrics
            .total_failed_messages
            .with_label_values(&["chain_id_1", "block_producer_1"])
            .inc_by(3);

        // Gather all the metrics
        let metric_families = gather();
        let mut buffer = Vec::new();
        let encoder = TextEncoder::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();

        // Convert the gathered output to a string
        let output = String::from_utf8(buffer.clone()).unwrap();

        // Assert that the output contains the correct failed message metric
        assert!(output.contains("total_failed_messages"));
        assert!(output.contains("chain_id_1"));
        assert!(output.contains("block_producer_1"));
        assert!(output.contains("3"));
    }

    #[test]
    fn test_total_aborted_signing_sessions_metric() {
        let metrics = BtcServerMetrics::random();

        metrics.total_aborted_signing_sessions.with_label_values(&["regtest", "5"]).set(10);

        let metric_families = gather();
        let mut buffer = Vec::new();
        let encoder = TextEncoder::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();

        let output = String::from_utf8(buffer.clone()).unwrap();

        assert!(output.contains("total_aborted_signing_sessions"));
        assert!(output.contains("regtest"));
        assert!(output.contains("5"));
    }

    #[test]
    fn test_round1_signing_throughput_metric() {
        let metrics = BtcServerMetrics::random();

        metrics.round1_signing_throughput.with_label_values(&["regtest", "3", "abc"]).inc_by(10);

        let metric_families = gather();
        let mut buffer = Vec::new();
        let encoder = TextEncoder::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();

        let output = String::from_utf8(buffer.clone()).unwrap();

        assert!(output.contains("round1_signing_throughput"));
        assert!(output.contains("regtest"));
        assert!(output.contains("3"));
        assert!(output.contains("abc"));
    }

    #[test]
    fn test_error_rates_metric() {
        let metrics = BtcServerMetrics::random();

        metrics
            .signing_error_rates
            .with_label_values(&["regtest", "4", "xyz", "write_error"])
            .inc_by(1);

        let metric_families = gather();
        let mut buffer = Vec::new();
        let encoder = TextEncoder::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();

        let output = String::from_utf8(buffer.clone()).unwrap();

        assert!(output.contains("signing_error_rates"));
        assert!(output.contains("regtest"));
        assert!(output.contains("4"));
        assert!(output.contains("xyz"));
        assert!(output.contains("write_error"));
    }
}
