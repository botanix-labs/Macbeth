use super::models::*;
use reth_db::{tables, TableType, TableViewer};
use reth_primitives::{BlockNumber, B256};
use std::fmt;

tables! {
    /// Store snapshot id to snapshot data.
    table Snapshots<Key = SnapshotId, Value = Snapshot>;

    /// Store wallet state sync record id to wallet state sync data.
    table WalletStateSyncs<Key = PeerID, Value = WalletStateSyncRecord>;

    /// Store staged headers, used to persist pegins and pegouts after
    /// finalizing a block.
    table StagedHeaders<Key = B256, Value = HeaderWithPegs>;

    /// Store chunk id to chunk data.
    table Chunks<Key = ChunkId, Value = SnapshotChunk>;

    /// Stores block number to snapshot id.
    table BlockSnapshots<Key = BlockNumber, Value = SnapshotId>;

    /// Stores the chunk to Block ids
    table ChunkBlocks<Key = ChunkId, Value = BlockNumber>;

    /// Table used when syncing snapshots.
    table SnapshotSyncs<Key = SnapshotSyncId, Value = SnapshotSync>;
}
