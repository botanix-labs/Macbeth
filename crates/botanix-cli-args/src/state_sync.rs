use clap::Args;

/// The default number of recent snapshots to keep.
pub const DEFAULT_NUM_SNAPSHOTS_TO_KEEP: u64 = 3;

/// Snapshot message format for state sync prod
pub const SNAPSHOT_MESSAGE_FORMAT: u32 = 1;

#[allow(dead_code)]
/// Snapshot message format for state sync test
pub const SNAPSHOT_MESSAGE_FORMAT_TEST: u32 = 2;

/// Wallet state sync chunk size
pub const WALLET_STATE_SYNC_CHUNK_SIZE: u64 = 10;
/// Parameters to configure state sync.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[clap(next_help_heading = "sync")]
pub struct StateSyncArgs {
    /// Snapshot keep recent.
    ///
    /// The snapshot keep recent.
    #[arg(default_value_t=DEFAULT_NUM_SNAPSHOTS_TO_KEEP, long = "sync.num_snapshots_to_keep", name = "sync.num_snapshots_to_keep", value_name = "NUM_SNAPSHOTS_TO_KEEP", env = "RETH_SYNC_NUM_SNAPSHOTS_TO_KEEP")]
    pub num_snapshots_to_keep: u64,

    /// Snapshot message format
    #[arg(default_value_t=SNAPSHOT_MESSAGE_FORMAT, long = "sync.snapshot_message_format", name = "sync.snapshot_message_format", value_name = "SNAPSHOT_MESSAGE_FORMAT", env = "RETH_SYNC_SNAPSHOT_MESSAGE_FORMAT")]
    pub snapshot_message_format: u32,

    /// State sync enabled
    #[arg(
        default_value_t = false,
        long = "sync.enable_state_sync",
        name = "sync.enable_state_sync",
        value_name = "ENABLE_STATE_SYNC",
        env = "RETH_SYNC_ENABLE_STATE_SYNC"
    )]
    pub enable_state_sync: bool,

    /// Historical state sync enabled
    #[arg(
        default_value_t = false,
        long = "sync.enable_historical_sync",
        name = "sync.enable_historical_sync",
        value_name = "ENABLE_HISTORICAL_SYNC",
        env = "RETH_SYNC_ENABLE_HISTORICAL_SYNC"
    )]
    pub enable_historical_sync: bool,

    /// Wallet state sync chunk size
    #[arg(default_value_t=WALLET_STATE_SYNC_CHUNK_SIZE, long = "sync.wallet_state_sync_chunk_size", name = "sync.wallet_state_sync_chunk_size", value_name = "WALLET_STATE_SYNC_CHUNK_SIZE", env = "RETH_SYNC_WALLET_STATE_SYNC_CHUNK_SIZE")]
    pub wallet_state_sync_chunk_size: u64,
}

impl Default for StateSyncArgs {
    fn default() -> Self {
        Self {
            num_snapshots_to_keep: DEFAULT_NUM_SNAPSHOTS_TO_KEEP,
            snapshot_message_format: SNAPSHOT_MESSAGE_FORMAT,
            enable_state_sync: true,
            enable_historical_sync: true,
            wallet_state_sync_chunk_size: WALLET_STATE_SYNC_CHUNK_SIZE,
        }
    }
}
