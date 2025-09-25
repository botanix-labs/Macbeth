use prometheus::{
    register_gauge_vec, register_histogram_vec, register_int_counter_vec, register_int_gauge_vec,
    GaugeVec, HistogramVec, IntCounterVec, IntGaugeVec, Registry,
};

use crate::version::{
    CARGO_PKG_VERSION, VERGEN_BUILD_TIMESTAMP, VERGEN_GIT_SHA, VERGEN_RUSTC_SEMVER,
};

#[macro_export]
macro_rules! update_pegout_scheduler_error_metrics {
    ($telemetry:expr, $btc_network:expr, $self_id:expr, $error:expr) => {
        if let Some(telemetry) = $telemetry {
            telemetry.update_pegout_scheduler_error_metrics(
                $btc_network,
                $self_id,
                &format!("{}", $error),
            );
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

#[macro_export]
macro_rules! handle_signing_error {
    // Pattern 1: 3 args - return value on success
    ($self:expr, $operation:expr) => {
        match $operation.to_status() {
            Ok(value) => value,
            Err(e) => {
                if let Some(telemetry) = $self.telemetry.as_ref() {
                    telemetry.update_signing_error_metrics(
                        $self.btc_network,
                        $self.config.identifier,
                        &e.to_string(),
                    );
                }
                return Err(e);
            }
        }
    };

    // Pattern 2: 3 args but with explicit signing_session_id - just check for errors
    ($self:expr, $operation:expr, check_only) => {
        if let Err(e) = $operation.to_status() {
            if let Some(telemetry) = $self.telemetry.as_ref() {
                telemetry.update_signing_error_metrics(
                    $self.btc_network,
                    $self.config.identifier,
                    &e.to_string(),
                );
            }
            return Err(e);
        }
    };
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
    pub round1_signing_throughput: IntCounterVec,
    pub round2_signing_throughput: IntCounterVec,
    pub round1_signing_package_size_histogram: HistogramVec,
    pub round2_signing_package_size_histogram: HistogramVec,

    // Wallet and UTXO Management Metrics
    pub pegin_utxos_count: IntGaugeVec,
    pub pegin_utxos_total_value: IntGaugeVec,
    pub pegout_utxos_count: IntGaugeVec,
    pub pegout_utxos_total_value: IntGaugeVec,
    pub input_selection_time: IntCounterVec, // TODO (to be done once Darius's PR is merged)

    pub pegins_count: IntCounterVec,
    pub pegouts_count: IntCounterVec,
    pub success_broadcasted_pegout_txs_count: IntGaugeVec,
    pub failed_broadcasted_pegout_txs_count: IntGaugeVec,
    pub started_round1_signings_count: IntGaugeVec,
    pub completed_round2_signings_count: IntGaugeVec,

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
    pub transaction_fee_rates: HistogramVec,
    pub fee_rate_abnormalities: IntCounterVec,

    pub last_attempted_pegout_height: IntGaugeVec,
    pub last_successful_pegout_height: IntGaugeVec,
    pub last_pegin_height: IntGaugeVec,

    // version and config-related metrics
    pub info: GaugeVec,
    pub config: GaugeVec,
}

impl Default for BtcServerMetrics {
    fn default() -> Self {
        BtcServerMetrics::new(None).expect("Failed to create default BtcServerMetrics")
    }
}

impl BtcServerMetrics {
    pub fn new(prefix: Option<String>) -> anyhow::Result<Self> {
        let metric_prefix = prefix.clone().unwrap_or("btc_server".to_string());
        println!("Initializing metrics registry with prefix: {}", metric_prefix);

        // ================================== signing ==================================
        let total_signing_sessions = register_int_gauge_vec!(
            "total_signing_sessions",
            "A metric counting the number of total signing sessions",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let total_aborted_signing_sessions = register_int_gauge_vec!(
            "total_aborted_signing_sessions",
            "A metric counting the number of total aborted signing sessions",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let total_finalized_signing_sessions = register_int_gauge_vec!(
            "total_finalized_signing_sessions",
            "A metric counting the number of total finalized signing sessions",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let total_received_round1_signing_packages = register_int_counter_vec!(
            "total_received_round1_signing_packages",
            "A metric counting the number of received round 1 packages",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let total_received_round2_signing_packages = register_int_counter_vec!(
            "total_received_round2_signing_packages",
            "A metric counting the number of received round 2 packages",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let round1_signing_throughput = register_int_counter_vec!(
            "round1_signing_throughput",
            "A metric counting the number of gossiped round1 signing messages per signing round and id",
            &["btc_chain", "self_id", "signing_session_id"],
        )
        .expect("metric must be created");

        let round2_signing_throughput = register_int_counter_vec!(
            "round2_signing_throughput",
            "A metric counting the number of gossiped round2 signing messages per signing round and id",
            &["btc_chain", "self_id", "signing_session_id"],
        )
        .expect("metric must be created");

        // New histogram metric for block latency
        let round1_signing_latency = register_histogram_vec!(
            "round1_signing_latency_ms",
            "Histogram of latencies between receiving and writing round1 signing package to db",
            &["btc_chain", "self_id"],
            // buckets for latency measurement in ms (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0, 10000.0, 100000.0, 1000000.0],
        )
        .expect("metric must be created");

        let round2_signing_latency = register_histogram_vec!(
            "round2_signing_latency_ms",
            "Histogram of latencies between receiving and writing round2 signing package to db",
            &["btc_chain", "self_id"],
            // buckets for latency measurement in ms (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0, 10000.0, 100000.0, 1000000.0],
        )
        .expect("metric must be created");

        let round1_signing_package_size_histogram = register_histogram_vec!(
            "round1_signing_package_size_bytes",
            "Histogram of round1 signing packages sizes in bytes",
            &["btc_chain", "self_id"],
            vec![10.0, 100.0, 500.0, 1000.0, 5000.0, 10000.0, 100000.0, 1000000.0]
        )
        .expect("metric must be created");

        let round2_signing_package_size_histogram = register_histogram_vec!(
            "round2_signing_package_size_bytes",
            "Histogram of round2 signing packages sizes in bytes",
            &["btc_chain", "self_id"],
            vec![10.0, 100.0, 500.0, 1000.0, 5000.0, 10000.0, 100000.0, 1000000.0]
        )
        .expect("metric must be created");

        let pegin_utxos_count = register_int_gauge_vec!(
            "pegin_utxos_count",
            "A metric counting the number of pegin UTXOs",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let pegin_utxos_total_value = register_int_gauge_vec!(
            "pegin_utxos_total_value",
            "A metric representing the total value of pegin UTXOs in satoshis",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let pegout_utxos_count = register_int_gauge_vec!(
            "pegout_utxos_count",
            "A metric counting the number of pegout UTXOs",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let pegout_utxos_total_value = register_int_gauge_vec!(
            "pegout_utxos_total_value",
            "A metric representing the total value of pegout UTXOs in satoshis",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let input_selection_time = register_int_counter_vec!(
            "input_selection_time",
            "A metric counting the time taken for input selection",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let pegins_count = register_int_counter_vec!(
            "pegins_count",
            "A metric counting the pegins",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let pegouts_count = register_int_counter_vec!(
            "pegouts_count",
            "A metric counting the pegouts",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let success_broadcasted_pegout_txs_count = register_int_gauge_vec!(
            "success_broadcasted_pegout_txs_count",
            "A metric counting the scuccessful broadcasted pegout txs",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let failed_broadcasted_pegout_txs_count = register_int_gauge_vec!(
            "failed_broadcasted_pegout_txs_count",
            "A metric counting the failed broadcasted pegout txs",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let started_round1_signings_count = register_int_gauge_vec!(
            "started_round1_signings_count",
            "A metric counting the started round1 signings",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let completed_round2_signings_count = register_int_gauge_vec!(
            "completed_round2_signings_count",
            "A metric counting the completed round2 signings",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let signing_error_rates = register_int_counter_vec!(
            "signing_error_rates",
            "A metric counting errors or failures during signing message processing",
            &["btc_chain", "self_id", "error_type"],
        )
        .expect("metric must be created");

        let signing_success_rate = register_int_counter_vec!(
            "signing_success_rate",
            "A metric counting successfully signed messages",
            &["btc_chain", "self_id", "signing_session_id"],
        )
        .expect("metric must be created");

        let member_uptime = register_int_gauge_vec!(
            "member_uptime",
            "A metric counting the uptime of federation members",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        // System Performance Metrics
        let bitcoind_rpc_latency = register_histogram_vec!(
            "bitcoind_rpc_latency",
            "A metric representing the latency of bitcoind RPC calls",
            &["btc_chain", "self_id", "rpc_method"],
            vec![10.0, 100.0, 500.0, 1000.0, 5000.0, 10000.0, 100000.0, 1000000.0]
        )
        .expect("metric must be created");

        let bitcoind_sync_status = register_int_gauge_vec!(
            "bitcoind_sync_status",
            "A metric representing the sync status of bitcoind",
            &["btc_chain", "self_id", "service"] // status can be "syncing" = 0 or "up" = 1
        )
        .expect("metric must be created");

        let fee_rate_abnormalities = register_int_counter_vec!(
            "fee_rate_abnormalities",
            "A metric counting fee rate abnormalities",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        //  ================================== dkg ==================================
        let total_received_round1_dkg_packages = register_int_counter_vec!(
            "total_received_round1_dkg_packages",
            "A metric counting the number of received round 1 dkg packages",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let total_received_round2_dkg_packages = register_int_counter_vec!(
            "total_received_round2_dkg_packages",
            "A metric counting the number of received round 2 dkg packages",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let total_received_round3_dkg_packages = register_int_counter_vec!(
            "total_received_round3_dkg_packages",
            "A metric counting the number of received round 3 dkg packages",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        // ---
        let round1_dkg_throughput = register_int_counter_vec!(
            "round1_dkg_throughput",
            "A metric counting the number of gossiped round1 dkg messages per id",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let round2_dkg_throughput = register_int_counter_vec!(
            "round2_dkg_throughput",
            "A metric counting the number of gossiped round2 dkg messages per id",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let round3_dkg_throughput = register_int_counter_vec!(
            "round3_dkg_throughput",
            "A metric counting the number of gossiped round2 dkg messages per id",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        // ---
        // New histogram metric for package latency
        let round1_dkg_latency_histogram = register_histogram_vec!(
            "round1_dkg_latency_secs",
            "Histogram of latencies between receiving and writing dkg package to db",
            &["btc_chain", "self_id"],
            // buckets for latency measurement (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0],
        )
        .expect("metric must be created");

        let round2_dkg_latency_histogram = register_histogram_vec!(
            "round2_dkg_latency_secs",
            "Histogram of latencies between receiving and writing round2 dkg package to db",
            &["btc_chain", "self_id"],
            // buckets for latency measurement (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0],
        )
        .expect("metric must be created");

        let round3_dkg_latency_histogram = register_histogram_vec!(
            "round3_dkg_latency_secs",
            "Histogram of latencies between receiving and writing round2 dkg package to db",
            &["btc_chain", "self_id"],
            // buckets for latency measurement (e.g., 0.1s, 0.5s, 1s, 5s, 10s)
            vec![10.0, 50.0, 100.0, 500.0, 1000.0],
        )
        .expect("metric must be created");

        // ---
        let round1_dkg_package_size_histogram = register_histogram_vec!(
            "round1_dkg_package_size_bytes",
            "Histogram of round1 dkg packages sizes in bytes",
            &["btc_chain", "self_id"],
            vec![10.0, 100.0, 500.0, 1000.0, 5000.0, 10000.0, 100000.0, 1000000.0]
        )
        .expect("metric must be created");

        let round2_dkg_package_size_histogram = register_histogram_vec!(
            "round2_dkg_package_size_bytes",
            "Histogram of round2 dkg packages sizes in bytes",
            &["btc_chain", "self_id"],
            vec![10.0, 100.0, 500.0, 1000.0, 5000.0, 10000.0, 100000.0, 1000000.0]
        )
        .expect("metric must be created");

        let dkg_error_rates = register_int_counter_vec!(
            "dkg_error_rates",
            "A metric counting errors or failures during dkg message processing",
            &["btc_chain", "self_id", "error_type"],
        )
        .expect("metric must be created");

        // ---
        let pegout_scheduler_error_rates = register_int_counter_vec!(
            "pegout_scheduler_error_rates",
            "A metric counting errors or failures during the pegout scheduler processing",
            &["btc_chain", "self_id", "error_type"],
        )
        .expect("metric must be created");

        // ====================================================================
        // Transaction Processing Metrics
        let pending_pegouts = register_int_gauge_vec!(
            "pending_pegouts",
            "A metric counting the number of pending pegouts",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let finalized_pegout_ids = register_int_gauge_vec!(
            "finalized_pegout_ids",
            "A metric counting the number of pending pegouts",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let pegin_confirmation_depth = register_int_gauge_vec!(
            "pegin_confirmation_depth",
            "A metric representing the confirmation depth of pegin transactions",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let transaction_fee_rates = register_histogram_vec!(
            "transaction_fee_rates",
            "A metric representing the transaction fee rates",
            &["btc_chain", "self_id"],
            // buckets for measurement in satoshis (e.g., 1.0, 100.0, 10000.0, 100000.0, 1000000.0,
            // 10000000.0, 100000000.0)
            vec![1.0, 10.0, 100.0, 10000.0, 100000.0, 1000000.0, 10000000.0, 100000000.0] // up to 1 BTC,
        )
        .expect("metric must be created");

        let last_attempted_pegout_height = register_int_gauge_vec!(
            "last_attempted_pegout_height",
            "A metric representing the last attempted pegout height",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let last_successful_pegout_height = register_int_gauge_vec!(
            "last_successful_pegout_height",
            "A metric representing the last successful pegout height",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        let last_pegin_height = register_int_gauge_vec!(
            "last_pegin_height",
            "A metric representing the last pegin height",
            &["btc_chain", "self_id"],
        )
        .expect("metric must be created");

        // ====================================================================
        let info = register_gauge_vec!(
            "info",
            "Application status information",
            &["version", "git_sha", "build_time", "rust_version"]
        )
        .expect("metric must be created");

        let config = register_gauge_vec!(
            "config",
            "Application configuration",
            &["btc_network", "identifier", "min_signers", "max_signers"]
        )
        .expect("metric must be created");

        info.with_label_values(&[
            CARGO_PKG_VERSION,
            VERGEN_GIT_SHA,
            VERGEN_BUILD_TIMESTAMP,
            VERGEN_RUSTC_SEMVER,
        ])
        .set(1f64);

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
        registry.register(Box::new(round1_signing_throughput.clone()))?;
        registry.register(Box::new(round2_signing_throughput.clone()))?;
        registry.register(Box::new(round1_signing_package_size_histogram.clone()))?;
        registry.register(Box::new(round2_signing_package_size_histogram.clone()))?;
        registry.register(Box::new(pegin_utxos_count.clone()))?;
        registry.register(Box::new(pegin_utxos_total_value.clone()))?;
        registry.register(Box::new(pegout_utxos_count.clone()))?;
        registry.register(Box::new(pegout_utxos_total_value.clone()))?;
        registry.register(Box::new(input_selection_time.clone()))?;
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

        registry.register(Box::new(pegins_count.clone()))?;
        registry.register(Box::new(pegouts_count.clone()))?;
        registry.register(Box::new(success_broadcasted_pegout_txs_count.clone()))?;
        registry.register(Box::new(failed_broadcasted_pegout_txs_count.clone()))?;

        // Transaction Processing Metrics
        registry.register(Box::new(pending_pegouts.clone()))?;
        registry.register(Box::new(finalized_pegout_ids.clone()))?;
        registry.register(Box::new(pegin_confirmation_depth.clone()))?;
        registry.register(Box::new(transaction_fee_rates.clone()))?;

        registry.register(Box::new(last_attempted_pegout_height.clone()))?;
        registry.register(Box::new(last_successful_pegout_height.clone()))?;
        registry.register(Box::new(last_pegin_height.clone()))?;

        // Config and version-related metrics
        registry.register(Box::new(info.clone()))?;
        registry.register(Box::new(config.clone()))?;

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
            round1_signing_throughput,
            round2_signing_throughput,
            round1_signing_package_size_histogram,
            round2_signing_package_size_histogram,
            total_received_round1_signing_packages,
            total_received_round2_signing_packages,
            pegin_utxos_count,
            pegin_utxos_total_value,
            pegout_utxos_count,
            pegout_utxos_total_value,
            input_selection_time,
            member_uptime,
            pegins_count,
            pegouts_count,
            success_broadcasted_pegout_txs_count,
            failed_broadcasted_pegout_txs_count,
            started_round1_signings_count,
            completed_round2_signings_count,
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
            transaction_fee_rates,

            // Config and version-related metrics
            last_attempted_pegout_height,
            last_successful_pegout_height,
            last_pegin_height,

            info,
            config,
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

        metrics.signing_error_rates.with_label_values(&["regtest", "4", "write_error"]).inc_by(1);

        let metric_families = gather();
        let mut buffer = Vec::new();
        let encoder = TextEncoder::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();

        let output = String::from_utf8(buffer.clone()).unwrap();

        assert!(output.contains("signing_error_rates"));
        assert!(output.contains("regtest"));
        assert!(output.contains("4"));
        assert!(output.contains("write_error"));
    }
}
