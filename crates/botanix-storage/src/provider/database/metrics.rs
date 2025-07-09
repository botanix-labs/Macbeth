use metrics::Histogram;
use reth_metrics::Metrics;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct BotanixDurationsRecorder {
    start: Instant,
    current_metrics: BotanixDatabaseProviderMetrics,
    pub(crate) actions: Vec<(Action, Duration)>,
    latest: Option<Duration>,
}

impl Default for BotanixDurationsRecorder {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            actions: Vec::new(),
            latest: None,
            current_metrics: BotanixDatabaseProviderMetrics::default(),
        }
    }
}

impl BotanixDurationsRecorder {
    /// Saves the provided duration for future logging and instantly reports as a metric with
    /// `action` label.
    pub(crate) fn record_duration(&mut self, action: Action, duration: Duration) {
        self.actions.push((action, duration));
        self.current_metrics.record_duration(action, duration);
        self.latest = Some(self.start.elapsed());
    }

    /// Records the duration since last record, saves it for future logging and instantly reports as
    /// a metric with `action` label.
    pub(crate) fn record_relative(&mut self, action: Action) {
        let elapsed = self.start.elapsed();
        let duration = elapsed - self.latest.unwrap_or_default();

        self.actions.push((action, duration));
        self.current_metrics.record_duration(action, duration);
        self.latest = Some(elapsed);
    }
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum Action {
    // TODO: add actions
}

/// Database provider metrics
#[derive(Metrics)]
#[metrics(scope = "storage.providers.database")]
struct BotanixDatabaseProviderMetrics {
    // TODO: Define metrics
}

impl BotanixDatabaseProviderMetrics {
    /// Records the duration for the given action.
    pub(crate) fn record_duration(&self, action: Action, duration: Duration) {
        match action {
            // TODO
            //Action::InsertStorageHashing => self.insert_storage_hashing.record(duration),
        }
    }
}
