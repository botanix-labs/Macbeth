mod activation_manager;
mod snapshot;
mod staged_header;
mod wallet_sync;

pub use activation_manager::*;
pub use snapshot::*;
pub use staged_header::*;
pub use wallet_sync::*;

use reth_codecs::Compact;
use reth_db_api::{
    impl_compression_for_compact,
    table::{Compress, Decompress},
};

impl_compression_for_compact!(
    Snapshot,
    SnapshotChunk,
    SnapshotSync,
    HeaderWithPegs,
    WalletStateSyncRecord
);
