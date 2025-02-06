use clap::Args;

/// The default maximum size for a snapshot in bytes (8 MB).
pub(crate) const DEFAULT_MAX_SNAPSHOT_SIZE_BYTES: usize = 8 * 1024 * 1024; // 8 Mbs max size
/// The default number of recent snapshots to keep.
pub(crate) const DEFAULT_NUM_SNAPSHOTS_TO_KEEP: u64 = 3;

/// Parameters to configure state sync.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[clap(next_help_heading = "sync")]
pub struct StateSyncArgs {
    /// Snapshot keep recent.
    ///
    /// The snapshot keep recent.
    #[arg(default_value_t=DEFAULT_NUM_SNAPSHOTS_TO_KEEP, long = "sync.num_snapshots_to_keep", name = "sync.num_snapshots_to_keep", value_name = "NUM_SNAPSHOTS_TO_KEEP")]
    pub num_snapshots_to_keep: u64,
}

impl Default for StateSyncArgs {
    fn default() -> Self {
        Self { num_snapshots_to_keep: DEFAULT_NUM_SNAPSHOTS_TO_KEEP }
    }
}
