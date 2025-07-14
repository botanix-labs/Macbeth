mod metrics;
mod system;

use log::error;
use metrics::BtcServerMetrics;
use parking_lot::RwLock;
use std::sync::Arc;
use system::{System, SystemMetricsWrapper};

#[derive(Clone)]
pub struct Telemetry {
    system: Arc<RwLock<System>>,
    btc_server_metrics: Option<Arc<BtcServerMetrics>>,
}

impl Telemetry {
    pub async fn new() -> anyhow::Result<Arc<Self>> {
        let system = Arc::new(RwLock::new(System::new().await));

        let btc_server_metrics = Some(Arc::new(BtcServerMetrics::default()));

        Ok(Arc::new(Self { system, btc_server_metrics }))
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        let system = Arc::clone(&self.system);
        tokio::spawn(async move {
            system.write().refresh();
        });
        Ok(())
    }

    pub fn record_bitcoind_rpc_latency(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        rpc_method: &str,
        latency_millis: u128,
    ) {
        self.maybe_use_metrics(|metrics| {
            // update latency histogram (in milliseconds)
            metrics
                .bitcoind_rpc_latency
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string(), rpc_method])
                .observe(latency_millis as f64);
        });
    }

    pub fn update_round1_signing_metrics(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        session_id: &[u8; 32],
        data_size: usize,
        latency_millis: u128,
    ) {
        self.maybe_use_metrics(|metrics| {
            // Update package size histogram
            metrics
                .round1_signing_package_size_histogram
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(data_size as f64);

            // update latency histogram (in milliseconds)
            metrics
                .round1_signing_latency
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(latency_millis as f64);

            // Increment total received packages
            metrics
                .total_received_round1_signing_packages
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .inc();

            // Increment throughput for sessionid
            metrics
                .round1_signing_throughput
                .with_label_values(&[
                    &btc_chain.to_string(),
                    &self_id.to_string(),
                    &hex::encode(session_id),
                ])
                .inc();
        });
    }

    pub fn update_round2_signing_metrics(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        session_id: &[u8; 32],
        data_size: usize,
        latency_millis: u128,
    ) {
        self.maybe_use_metrics(|metrics| {
            // Update package size histogram
            metrics
                .round2_signing_package_size_histogram
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(data_size as f64);

            // update latency histogram
            metrics
                .round2_signing_latency
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(latency_millis as f64);

            // Increment total received packages
            metrics
                .total_received_round2_signing_packages
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .inc();

            // Increment throughput for sessionid
            metrics
                .round2_signing_throughput
                .with_label_values(&[
                    &btc_chain.to_string(),
                    &self_id.to_string(),
                    &hex::encode(session_id),
                ])
                .inc();
        });
    }

    pub fn update_round1_dkg_metrics(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        data_size: usize,
        latency_millis: u128,
    ) {
        self.maybe_use_metrics(|metrics| {
            // Update package size histogram
            metrics
                .round1_dkg_package_size_histogram
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(data_size as f64);

            // update latency histogram
            metrics
                .round1_dkg_latency_histogram
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(latency_millis as f64);

            // Increment total received packages
            metrics
                .total_received_round1_dkg_packages
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .inc();

            // Increment throughput for sessionid
            metrics
                .round1_dkg_throughput
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .inc();
        });
    }

    pub fn update_round2_dkg_metrics(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        data_size: usize,
        latency_millis: u128,
    ) {
        self.maybe_use_metrics(|metrics| {
            // Update package size histogram
            metrics
                .round2_dkg_package_size_histogram
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(data_size as f64);

            // update latency histogram
            metrics
                .round2_dkg_latency_histogram
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(latency_millis as f64);

            // Increment total received packages
            metrics
                .total_received_round2_dkg_packages
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .inc();

            // Increment throughput for sessionid
            metrics
                .round2_dkg_throughput
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .inc();
        });
    }

    pub fn update_round3_dkg_metrics(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        latency_millis: u128,
    ) {
        self.maybe_use_metrics(|metrics| {
            // update latency histogram
            metrics
                .round3_dkg_latency_histogram
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(latency_millis as f64);

            // Increment total received packages
            metrics
                .total_received_round3_dkg_packages
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .inc();

            // Increment throughput for sessionid
            metrics
                .round3_dkg_throughput
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .inc();
        });
    }

    pub fn update_dkg_error_metrics(&self, btc_chain: bitcoin::Network, self_id: u16, error: &str) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .dkg_error_rates
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string(), error])
                .inc();
        });
    }

    pub fn update_signing_success_rate_metrics(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        session_id: [u8; 32],
    ) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .signing_success_rate
                .with_label_values(&[
                    &btc_chain.to_string(),
                    &self_id.to_string(),
                    &hex::encode(session_id),
                ])
                .inc();
        });
    }

    pub fn update_signing_error_metrics(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        session_id: [u8; 32],
        error: &str,
    ) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .signing_error_rates
                .with_label_values(&[
                    &btc_chain.to_string(),
                    &self_id.to_string(),
                    &hex::encode(session_id),
                    error,
                ])
                .inc();
        });
    }

    pub fn update_pegout_scheduler_error_metrics(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        error: &str,
    ) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .pegout_scheduler_error_rates
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string(), error])
                .inc();
        });
    }

    pub fn record_aborted_signing_sessions(&self, btc_chain: bitcoin::Network, self_id: u16) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .total_aborted_signing_sessions
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .inc();
        });
    }

    pub fn record_total_signing_sessions(&self, btc_chain: bitcoin::Network, self_id: u16) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .total_signing_sessions
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .inc();
        });
    }

    pub fn record_finalized_signing_sessions(&self, btc_chain: bitcoin::Network, self_id: u16) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .total_finalized_signing_sessions
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .inc();
        });
    }

    pub fn update_pending_pegouts(&self, btc_chain: bitcoin::Network, self_id: u16, pegouts: i64) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .pending_pegouts
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .add(pegouts);
        });
    }

    pub fn update_utxos(&self, btc_chain: bitcoin::Network, self_id: u16, utxos: i64) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .utxo_count
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .add(utxos);
        });
    }

    pub fn update_health_check(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        upstream_time: u64,
        service_status: &[(&str, &str)],
    ) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .member_uptime
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .set(upstream_time as i64);

            service_status.iter().for_each(|(service, status)| {
                metrics
                    .bitcoind_sync_status
                    .with_label_values(&[&btc_chain.to_string(), &self_id.to_string(), service])
                    .set(if *status == "up" { 1_i64 } else { 0_i64 });
            });
        });
    }

    pub fn set_pending_pegouts(&self, btc_chain: bitcoin::Network, self_id: u16, pegouts: i64) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .pending_pegouts
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .set(pegouts);
        });
    }

    pub fn update_finalized_pegout_ids(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        pegout_ids: i64,
    ) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .finalized_pegout_ids
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .add(pegout_ids);
        });
    }

    pub fn update_pegin_confirmation_depth(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        pegin_confirmation_depth: u32,
    ) {
        self.maybe_use_metrics(|metrics| {
            // Set pegin confirmation depth
            metrics
                .pegin_confirmation_depth
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .set(pegin_confirmation_depth as i64);
        });
    }

    pub fn update_transaction_fee_rates(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        transaction_fee_rate: f64,
    ) {
        self.maybe_use_metrics(|metrics| {
            // Set pegin confirmation depth
            metrics
                .transaction_fee_rates
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(transaction_fee_rate);
        });
    }

    pub fn update_fee_rate_abnormalities(&self, btc_chain: bitcoin::Network, self_id: u16) {
        self.maybe_use_metrics(|metrics| {
            // Set pegin confirmation depth
            metrics
                .fee_rate_abnormalities
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .inc();
        });
    }

    pub fn maybe_use_metrics<F>(&self, f: F)
    where
        F: Fn(&BtcServerMetrics),
    {
        if let Some(metrics) = &self.btc_server_metrics {
            f(metrics);
        }
    }

    /// Returns the collected metrics as a string.
    pub async fn get_metrics(&self) -> String {
        use prometheus::Encoder;
        let encoder = prometheus::TextEncoder::new();

        if self.btc_server_metrics.is_none() {
            return "".to_string();
        }

        // fetch all measured metrics
        let mut buffer = Vec::new();
        if let Err(e) = encoder
            .encode(&self.btc_server_metrics.as_ref().unwrap().registry.gather(), &mut buffer)
        {
            error!("could not encode custom metrics: {}", e);
        };
        let mut res = match String::from_utf8(buffer.clone()) {
            Ok(v) => v,
            Err(e) => {
                error!("custom metrics could not be from_utf8'd: {}", e);
                String::default()
            }
        };
        buffer.clear();

        let mut buffer = Vec::new();
        if let Err(e) = encoder.encode(&prometheus::gather(), &mut buffer) {
            error!("could not encode prometheus metrics: {}", e);
        };
        let res_custom = match String::from_utf8(buffer.clone()) {
            Ok(v) => v,
            Err(e) => {
                error!("prometheus metrics could not be from_utf8'd: {}", e);
                String::default()
            }
        };
        buffer.clear();

        res.push_str(&res_custom);

        // now fetch and add system metrics
        let system_metrics = match self.system.read().metrics() {
            Ok(m) => {
                let metrics = SystemMetricsWrapper::from(m);
                let labels: Vec<(&str, &str)> = vec![];
                match serde_prometheus::to_string(&metrics, None, labels) {
                    Ok(m) => m,
                    Err(err) => {
                        error!("could not encode system metrics: {:?}", err);
                        String::default()
                    }
                }
            }
            Err(err) => {
                error!("prometheus system metrics could not be stringified: {:?}", err);
                String::default()
            }
        };
        res.push_str(&system_metrics);

        res
    }
}
