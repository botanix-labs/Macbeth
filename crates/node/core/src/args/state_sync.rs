use clap::Args;

/// The default maximum size for a snapshot in bytes (8 MB).
pub(crate) const DEFAULT_MAX_SNAPSHOT_SIZE_BYTES: usize = 8 * 1024 * 1024; // 8 Mbs max size
/// The default size for a snapshot chunk in bytes (1 MB).
pub(crate) const DEFAULT_SNAPSHOT_CHUNK_SIZE_BYTES: usize = 1 * 1024 * 1024; // 1 MB
/// The default number of recent snapshots to keep.
pub(crate) const DEFAULT_SNAPSHOT_KEEP_RECENT: u64 = 3;

/// Parameters to configure state sync.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[clap(next_help_heading = "sync")]
pub struct StateSyncArgs {
    /// State Sync Arguments

    /// Max snapshot size bytes
    ///
    /// The maximum snapshot size in bytes.
    #[arg(default_value_t=DEFAULT_MAX_SNAPSHOT_SIZE_BYTES, long = "sync.max_snapshot_size_bytes", name = "sync.max_snapshot_size_bytes", value_name = "MAX_SNAPSHOT_SIZE_BYTES")]
    pub max_snapshot_size_bytes: usize,

    /// Snapshot Chunk Syze Bytes
    ///
    /// The snapshot chunk size in bytes.
    #[arg(default_value_t=DEFAULT_SNAPSHOT_CHUNK_SIZE_BYTES, long = "sync.snapshot_chunk_size_bytes", name = "sync.snapshot_chunk_size_bytes", value_name = "SNAPSHOT_CHUNK_SIZE_BYTES")]
    pub snapshot_chunk_size_bytes: usize,

    /// Snapshot keep recent.
    ///
    /// The snapshot keep recent.
    #[arg(default_value_t=DEFAULT_SNAPSHOT_KEEP_RECENT, long = "sync.snapshot_keep_recent", name = "sync.snapshot_keep_recent", value_name = "SNAPSHOT_KEEP_RECENT")]
    pub snapshot_keep_recent: u64,
}

impl Default for StateSyncArgs {
    fn default() -> Self {
        Self {
            max_snapshot_size_bytes: DEFAULT_MAX_SNAPSHOT_SIZE_BYTES,
            snapshot_chunk_size_bytes: DEFAULT_SNAPSHOT_CHUNK_SIZE_BYTES,
            snapshot_keep_recent: DEFAULT_SNAPSHOT_KEEP_RECENT,
        }
    }
}
