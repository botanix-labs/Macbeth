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
    pub main_process_healthy: bool,
    pub last_heartbeat_seconds_ago: u64,
}

#[derive(Clone)]
pub struct ServerState {
    pub telemetry: Arc<Telemetry>,
    pub start_time: Instant,
    pub connection_count: Arc<RwLock<u32>>,
    pub last_main_process_heartbeat: Arc<RwLock<Instant>>,
}

impl ServerState {
    pub async fn new(telemetry: Arc<Telemetry>) -> Self {
        Self { 
            start_time: Instant::now(), 
            connection_count: Arc::new(RwLock::new(0)), 
            telemetry,
            last_main_process_heartbeat: Arc::new(RwLock::new(Instant::now()))
        }
    }
    
    pub fn update_main_process_heartbeat(&self) {
        *self.last_main_process_heartbeat.write() = Instant::now();
    }
    
    pub fn get_main_process_health(&self) -> bool {
        // Consider main process healthy if we received a heartbeat within last 30 seconds
        self.last_main_process_heartbeat.read().elapsed() < Duration::from_secs(30)
    }
}

impl ServerState {
    pub fn is_healthy(&self) -> bool {
        // Health check now includes main process health
        self.get_main_process_health()
    }

    pub async fn get_health(&self) -> HealthResponse {
        let last_heartbeat_elapsed = self.last_main_process_heartbeat.read().elapsed();
        HealthResponse { 
            uptime: self.uptime().as_secs(),
            main_process_healthy: self.get_main_process_health(),
            last_heartbeat_seconds_ago: last_heartbeat_elapsed.as_secs()
        }
    }

    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }
}
