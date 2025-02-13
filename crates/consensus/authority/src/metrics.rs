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

    /// Number of finalzied signings
    pub(crate) finalized_signings: Counter,

    #[allow(dead_code)]
    /// Number of reset wallet states
    pub(crate) reset_wallet_states: Counter,

    /// Number of commet finalzied blocks
    pub(crate) commet_finalzied_blocks: Counter,

    /// Number of commet committed blocks
    pub(crate) commet_committed_blocks: Counter,

    /// Number of commet prepared proposals
    pub(crate) commet_prepared_proposals: Counter,

    /// Number of commet checked txs
    pub(crate) commet_checked_txs: Counter,

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
