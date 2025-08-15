use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::telemetry::Telemetry;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub uptime: u64,
}

#[derive(Clone)]
pub struct ServerState {
    pub telemetry: Arc<Telemetry>,
    pub start_time: Instant,
    pub connection_count: Arc<RwLock<u32>>,
}

impl ServerState {
    pub async fn new(telemetry: Arc<Telemetry>) -> Self {
        Self { start_time: Instant::now(), connection_count: Arc::new(RwLock::new(0)), telemetry }
    }
}

impl ServerState {
    pub fn is_healthy(&self) -> bool {
        true
    }

    pub async fn get_health(&self) -> HealthResponse {
        HealthResponse { uptime: self.uptime().as_secs() }
    }

    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }
}
