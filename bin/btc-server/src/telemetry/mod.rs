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

    pub fn update_round1_signing_metrics(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        session_id: &[u8; 32],
        data_size: usize,
        latency: u128,
    ) {
        self.maybe_use_metrics(|metrics| {
            // Update package size histogram
            metrics
                .round1_signing_package_size_histogram
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(data_size as f64);

            // update latency histogram
            metrics
                .round1_signing_latency_histogram
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(latency as f64);

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
        latency: u128,
    ) {
        self.maybe_use_metrics(|metrics| {
            // Update package size histogram
            metrics
                .round2_signing_package_size_histogram
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(data_size as f64);

            // update latency histogram
            metrics
                .round2_signing_latency_histogram
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .observe(latency as f64);

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
        latency: u128,
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
                .observe(latency as f64);

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
        latency: u128,
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
                .observe(latency as f64);

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

    pub fn update_dkg_error_metrics(&self, btc_chain: bitcoin::Network, self_id: u16, error: &str) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .dkg_error_rates
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string(), error])
                .inc();
        });
    }

    pub fn update_signing_error_metrics(
        &self,
        btc_chain: bitcoin::Network,
        self_id: u16,
        session_id: Option<[u8; 32]>,
        error: &str,
    ) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .signing_error_rates
                .with_label_values(&[
                    &btc_chain.to_string(),
                    &self_id.to_string(),
                    &session_id.map(hex::encode).unwrap_or_default(),
                    error,
                ])
                .inc();
        });
    }

    pub fn update_pegout_scheduler_error_metrics(&self, error: &str) {
        self.maybe_use_metrics(|metrics| {
            metrics.pegout_scheduler_error_rates.with_label_values(&[error]).inc();
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

    pub fn record_finalized_signing_sessions(&self, btc_chain: bitcoin::Network, self_id: u16) {
        self.maybe_use_metrics(|metrics| {
            metrics
                .total_finalized_signing_sessions
                .with_label_values(&[&btc_chain.to_string(), &self_id.to_string()])
                .inc();
        });
    }

    pub fn update_pending_pegouts(&self, pegouts: i64) {
        self.maybe_use_metrics(|metrics| {
            metrics.pending_pegouts.with_label_values(&[]).set(pegouts);
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
