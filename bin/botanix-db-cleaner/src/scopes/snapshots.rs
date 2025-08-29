use botanix_storage::{
    models::ChunkId, BotanixProviderFactory, DatabaseProviderFactoryRW, SnapshotReader,
    SnapshotWriter,
};
use reth_db::DatabaseEnv;
use reth_node_core::primitives::BlockNumber;
use std::{ops::RangeInclusive, sync::Arc};

/// Truncates all snapshots from the database.
/// This includes removing all chunks and blocks associated with the snapshots.
/// It also removes the snapshots themselves.
pub fn truncate(provider_factory: &BotanixProviderFactory<Arc<DatabaseEnv>>) -> anyhow::Result<()> {
    let provider_rw = provider_factory.provider_rw()?;

    for snapshot in provider_rw.get_snapshots()?.iter() {
        let snapshot_id = snapshot.id();
        tracing::info!(target: "db_cleaner::cli", "Processing snapshot {snapshot_id} ...");

        // === Chunks ===
        let mut chunk_ids = snapshot.chunk_ids().iter().cloned().collect::<Vec<ChunkId>>();
        chunk_ids.sort();
        if let (Some(start), Some(end)) = (chunk_ids.first(), chunk_ids.last()) {
            let chunks_range: RangeInclusive<u64> = *start..=*end;
            tracing::info!(target: "db_cleaner::cli", "Removing chunks from range {:?} for snapshot id - {snapshot_id}", chunks_range);
            provider_rw.remove_chunks(chunks_range.clone())?;
            provider_rw.delete_chunks_in_blocks(chunks_range)?;
        }

        // === Blocks ===
        let mut block_ids = snapshot.block_ids().iter().cloned().collect::<Vec<BlockNumber>>();
        block_ids.sort();
        if let (Some(start), Some(end)) = (block_ids.first(), block_ids.last()) {
            let blocks_range: RangeInclusive<BlockNumber> = *start..=*end;
            tracing::info!(target: "db_cleaner::cli", "Removing blocks range {:?} for snapshot id - {snapshot_id}", blocks_range);
            provider_rw.remove_block_snapshot_id_mapping(blocks_range)?;
        }

        // === Snapshot ===
        provider_rw.remove_snapshots(snapshot.id()..=snapshot.id())?;
    }

    provider_rw.commit()?;
    Ok(())
}
