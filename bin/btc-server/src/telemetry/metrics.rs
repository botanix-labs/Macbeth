use prometheus::{
    register_histogram_vec, register_int_counter_vec, register_int_gauge_vec, HistogramVec,
    IntCounterVec, IntGaugeVec, Registry,
};

#[macro_export]
macro_rules! update_pegout_scheduler_error_metrics {
    ($telemetry:expr, $error:expr) => {
        if let Some(telemetry) = $telemetry {
            telemetry.update_pegout_scheduler_error_metrics(&format!("{}", $error));
        }
    };
}

#[macro_export]
macro_rules! measure_rpc_latency {
    ($telemetry:expr, $btc_network:expr, $self_id:expr, $rpc_method:expr, $rpc_call:expr) => {{
        let start = std::time::Instant::now();
        let result = $rpc_call;
        let duration = start.elapsed();

        if let Some(telemetry) = $telemetry {
            telemetry.record_bitcoind_rpc_latency(
                $btc_network,
                $self_id,
                $rpc_method,
                duration.as_millis(),
            );
        }

        result
    }};

    // Version with error conversion
    ($telemetry:expr, $btc_network:expr, $self_id:expr, $rpc_method:expr, $rpc_call:expr, $error_mapper:expr) => {{
        let start = std::time::Instant::now();
        let result = $rpc_call.map_err($error_mapper);
        let duration = start.elapsed();

        if let Some(telemetry) = $telemetry {
            telemetry.record_bitcoind_rpc_latency(
                $btc_network,
                $self_id,
                $rpc_method,
                duration.as_millis(),
            );
        }

        result
    }};
}

#[derive(Clone, Debug)]
pub struct BtcServerMetrics {
    pub registry: Registry,

    // Signing Operation Metrics
    pub total_signing_sessions: IntGaugeVec,
    pub total_aborted_signing_sessions: IntGaugeVec,
    pub total_finalized_signing_sessions: IntGaugeVec,
    pub signing_error_rates: IntCounterVec,
    pub round1_signing_latency: HistogramVec,
    pub round2_signing_latency: HistogramVec,
    pub signing_success_rate: IntCounterVec,
    pub total_received_round1_signing_packages: IntCounterVec,
    pub total_received_round2_signing_packages: IntCounterVec,
    pub total_failed_messages: IntCounterVec,
    pub round1_signing_throughput: IntCounterVec,
    pub round2_signing_throughput: IntCounterVec,
    pub round1_signing_package_size_histogram: HistogramVec,
    pub round2_signing_package_size_histogram: HistogramVec,

    // Wallet and UTXO Management Metrics
    pub utxo_count: IntGaugeVec,
    pub utxo_value_distribution: HistogramVec, // TODO
    pub utxo_age_distribution: HistogramVec,   // TODO
    pub input_selection_time: IntCounterVec,   // TODO
    pub dust_utxo_count: IntGaugeVec,          // TODO

    // Federation Member Participation Metrics
    pub member_uptime: IntGaugeVec,

    // System Performance Metrics
    pub bitcoind_rpc_latency: HistogramVec,
    pub bitcoind_sync_status: IntGaugeVec,

    // Dkg Processing Metrics
    pub total_received_round1_dkg_packages: IntCounterVec,
    pub total_received_round2_dkg_packages: IntCounterVec,
    pub total_received_round3_dkg_packages: IntCounterVec,
    pub round1_dkg_throughput: IntCounterVec,
    pub round2_dkg_throughput: IntCounterVec,
    pub round3_dkg_throughput: IntCounterVec,
    pub round1_dkg_latency_histogram: HistogramVec,
    pub round2_dkg_latency_histogram: HistogramVec,
    pub round3_dkg_latency_histogram: HistogramVec,
    pub round1_dkg_package_size_histogram: HistogramVec,
    pub round2_dkg_package_size_histogram: HistogramVec,
    pub dkg_error_rates: IntCounterVec,

    // Pegout scheduler
    pub pegout_scheduler_error_rates: IntCounterVec,

    // Transaction Processing Metrics
    pub pending_pegouts: IntGaugeVec,
    pub finalized_pegout_ids: IntGaugeVec,
    pub pegin_confirmation_depth: IntGaugeVec,
    pub pegin_processing_latency: HistogramVec, // TODO
    pub pegout_completion_time: HistogramVec,   // TODO
    pub transaction_fee_rates: HistogramVec,
    pub fee_rate_abnormalities: IntCounterVec,
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
        let round1_signing_latency = register_histogram_vec!(
            format!("{}round1_signing_latency_ms", metric_prefix),
            "Histogram of latencies between receiving and writing round1 signing package to db",
            &["btc_chain", "self_id"],
            // buckets for latency measurement in ms (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0, 10000.0, 100000.0, 1000000.0],
        )
        .expect("metric must be created");

        let round2_signing_latency = register_histogram_vec!(
            format!("{}round2_signing_latency_ms", metric_prefix),
            "Histogram of latencies between receiving and writing round2 signing package to db",
            &["btc_chain", "self_id"],
            // buckets for latency measurement in ms (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0, 10000.0, 100000.0, 1000000.0],
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

        let utxo_count = register_int_gauge_vec!(
            format!("{}utxo_count", metric_prefix),
            "A metric counting the number of UTXOs",
            &["btc_chain"],
        )
        .expect("metric must be created");

        let utxo_value_distribution = register_histogram_vec!(
            format!("{}utxo_value_distribution", metric_prefix),
            "A metric representing the distribution of UTXO values",
            &["btc_chain"],
            vec![100.0, 500.0, 1000.0, 5000.0, 10000.0, 100000.0, 1000000.0]
        )
        .expect("metric must be created");

        let utxo_age_distribution = register_histogram_vec!(
            format!("{}utxo_age_distribution", metric_prefix),
            "A metric representing the distribution of UTXO ages",
            &["btc_chain"],
            vec![100.0, 500.0, 1000.0, 5000.0, 10000.0, 100000.0, 1000000.0]
        )
        .expect("metric must be created");

        let input_selection_time = register_int_counter_vec!(
            format!("{}input_selection_time", metric_prefix),
            "A metric counting the time taken for input selection",
            &["btc_chain"],
        )
        .expect("metric must be created");

        let dust_utxo_count = register_int_gauge_vec!(
            format!("{}dust_utxo_count", metric_prefix),
            "A metric counting the number of dust UTXOs",
            &["btc_chain"],
        )
        .expect("metric must be created");

        let signing_error_rates = register_int_counter_vec!(
            format!("{}signing_error_rates", metric_prefix),
            "A metric counting errors or failures during signing message processing",
            &["btc_chain", "self_id", "signing_session_id", "error_type"],
        )
        .expect("metric must be created");

        let signing_success_rate = register_int_counter_vec!(
            format!("{}signing_success_rate", metric_prefix),
            "A metric counting successfully signed messages",
            &["btc_chain", "self_id", "signing_session_id"],
        )
        .expect("metric must be created");

        let member_uptime = register_int_gauge_vec!(
            format!("{}member_uptime", metric_prefix),
            "A metric counting the uptime of federation members",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        // System Performance Metrics
        let bitcoind_rpc_latency = register_histogram_vec!(
            format!("{}bitcoind_rpc_latency", metric_prefix),
            "A metric representing the latency of bitcoind RPC calls",
            &["btc_chain", "self_id", "rpc_method"],
            vec![100.0, 500.0, 1000.0, 5000.0, 10000.0, 100000.0, 1000000.0]
        )
        .expect("metric must be created");

        let bitcoind_sync_status = register_int_gauge_vec!(
            format!("{}bitcoind_sync_status", metric_prefix),
            "A metric representing the sync status of bitcoind",
            &["btc_chain", "self_id", "service", "status"] // status can be "syncing" or "active"
        )
        .expect("metric must be created");

        let fee_rate_abnormalities = register_int_counter_vec!(
            format!("{}fee_rate_abnormalities", metric_prefix),
            "A metric counting fee rate abnormalities",
            &["btc_chain", "self_id"],
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

        let total_received_round3_dkg_packages = register_int_counter_vec!(
            format!("{}total_received_round3_dkg_packages", metric_prefix),
            "A metric counting the number of received round 3 dkg packages",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        // ---
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

        let round3_dkg_throughput = register_int_counter_vec!(
            format!("{}round3_dkg_throughput", metric_prefix),
            "A metric counting the number of gossiped round2 dkg messages per id",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        // ---
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

        let round3_dkg_latency_histogram = register_histogram_vec!(
            format!("{}round3_dkg_latency_secs", metric_prefix),
            "Histogram of latencies between receiving and writing round2 dkg package to db",
            &["btc_chain", "self_id"],
            // buckets for latency measurement (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0],
        )
        .expect("metric must be created");

        // ---
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

        // ---
        let pegout_scheduler_error_rates = register_int_counter_vec!(
            format!("{}pegout_scheduler_error_rates", metric_prefix),
            "A metric counting errors or failures during the pegout scheduler processing",
            &["error_type"],
        )
        .expect("metric must be created");

        // ====================================================================
        // Transaction Processing Metrics
        let pending_pegouts = register_int_gauge_vec!(
            format!("{}pending_pegouts", metric_prefix),
            "A metric counting the number of pending pegouts",
            &[],
        )
        .expect("metric must be created");

        let finalized_pegout_ids = register_int_gauge_vec!(
            format!("{}finalized_pegout_ids", metric_prefix),
            "A metric counting the number of pending pegouts",
            &[],
        )
        .expect("metric must be created");

        let pegin_confirmation_depth = register_int_gauge_vec!(
            format!("{}pegin_confirmation_depth", metric_prefix),
            "A metric representing the confirmation depth of pegin transactions",
            &[],
        )
        .expect("metric must be created");

        let pegin_processing_latency = register_histogram_vec!(
            format!("{}pegin_processing_latency", metric_prefix),
            "A metric representing the latency of pegin processing",
            &["btc_chain", "self_id"],
            // buckets for latency measurement (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0],
        )
        .expect("metric must be created");

        let pegout_completion_time = register_histogram_vec!(
            format!("{}pegout_completion_time", metric_prefix),
            "A metric representing the time taken to complete pegouts",
            &["btc_chain", "self_id"],
            // buckets for time measurement (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0],
        )
        .expect("metric must be created");

        let transaction_fee_rates = register_histogram_vec!(
            format!("{}transaction_fee_rates", metric_prefix),
            "A metric representing the transaction fee rates",
            &["btc_chain", "self_id"],
            // buckets for measurement in satoshis (e.g., 1.0, 100.0, 10000.0, 100000.0, 1000000.0,
            // 10000000.0, 100000000.0)
            vec![1.0, 100.0, 10000.0, 100000.0, 1000000.0, 10000000.0, 100000000.0] // up to 1 BTC,
        )
        .expect("metric must be created");

        // ====================================================================
        let registry = Registry::new_custom(prefix, None).expect("registry to be created");
        // Signing Operation Metrics
        registry.register(Box::new(total_signing_sessions.clone()))?;
        registry.register(Box::new(total_aborted_signing_sessions.clone()))?;
        registry.register(Box::new(total_finalized_signing_sessions.clone()))?;
        registry.register(Box::new(signing_error_rates.clone()))?;
        registry.register(Box::new(round1_signing_latency.clone()))?;
        registry.register(Box::new(round2_signing_latency.clone()))?;
        registry.register(Box::new(signing_success_rate.clone()))?;
        registry.register(Box::new(total_received_round1_signing_packages.clone()))?;
        registry.register(Box::new(total_received_round2_signing_packages.clone()))?;
        registry.register(Box::new(total_failed_messages.clone()))?;
        registry.register(Box::new(round1_signing_throughput.clone()))?;
        registry.register(Box::new(round2_signing_throughput.clone()))?;
        registry.register(Box::new(round1_signing_package_size_histogram.clone()))?;
        registry.register(Box::new(round2_signing_package_size_histogram.clone()))?;
        registry.register(Box::new(utxo_count.clone()))?;
        registry.register(Box::new(utxo_value_distribution.clone()))?;
        registry.register(Box::new(utxo_age_distribution.clone()))?;
        registry.register(Box::new(input_selection_time.clone()))?;
        registry.register(Box::new(dust_utxo_count.clone()))?;
        registry.register(Box::new(fee_rate_abnormalities.clone()))?;

        // Dkg Metrics
        registry.register(Box::new(total_received_round1_dkg_packages.clone()))?;
        registry.register(Box::new(total_received_round2_dkg_packages.clone()))?;
        registry.register(Box::new(total_received_round3_dkg_packages.clone()))?;
        registry.register(Box::new(round1_dkg_throughput.clone()))?;
        registry.register(Box::new(round2_dkg_throughput.clone()))?;
        registry.register(Box::new(round3_dkg_throughput.clone()))?;
        registry.register(Box::new(round1_dkg_latency_histogram.clone()))?;
        registry.register(Box::new(round2_dkg_latency_histogram.clone()))?;
        registry.register(Box::new(round3_dkg_latency_histogram.clone()))?;
        registry.register(Box::new(round1_dkg_package_size_histogram.clone()))?;
        registry.register(Box::new(round2_dkg_package_size_histogram.clone()))?;
        registry.register(Box::new(dkg_error_rates.clone()))?;
        registry.register(Box::new(pegout_scheduler_error_rates.clone()))?;
        registry.register(Box::new(member_uptime.clone()))?;
        registry.register(Box::new(bitcoind_rpc_latency.clone()))?;
        registry.register(Box::new(bitcoind_sync_status.clone()))?;

        // Transaction Processing Metrics
        registry.register(Box::new(pending_pegouts.clone()))?;
        registry.register(Box::new(finalized_pegout_ids.clone()))?;
        registry.register(Box::new(pegin_confirmation_depth.clone()))?;
        registry.register(Box::new(pegin_processing_latency.clone()))?;
        registry.register(Box::new(pegout_completion_time.clone()))?;
        registry.register(Box::new(transaction_fee_rates.clone()))?;

        Ok(Self {
            registry,
            // Signing Operation Metrics
            total_signing_sessions,
            total_aborted_signing_sessions,
            total_finalized_signing_sessions,
            signing_error_rates,
            round1_signing_latency,
            round2_signing_latency,
            signing_success_rate,
            total_failed_messages,
            round1_signing_throughput,
            round2_signing_throughput,
            round1_signing_package_size_histogram,
            round2_signing_package_size_histogram,
            total_received_round1_signing_packages,
            total_received_round2_signing_packages,
            utxo_count,
            utxo_value_distribution,
            utxo_age_distribution,
            input_selection_time,
            dust_utxo_count,
            member_uptime,
            bitcoind_rpc_latency,
            bitcoind_sync_status,
            fee_rate_abnormalities,
            total_received_round1_dkg_packages,
            total_received_round2_dkg_packages,
            total_received_round3_dkg_packages,
            round1_dkg_throughput,
            round2_dkg_throughput,
            round3_dkg_throughput,
            round1_dkg_latency_histogram,
            round2_dkg_latency_histogram,
            round3_dkg_latency_histogram,
            round1_dkg_package_size_histogram,
            round2_dkg_package_size_histogram,
            dkg_error_rates,
            pegout_scheduler_error_rates,

            // Transaction Processing Metrics
            pending_pegouts,
            finalized_pegout_ids,
            pegin_confirmation_depth,
            pegin_processing_latency,
            pegout_completion_time,
            transaction_fee_rates,
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
