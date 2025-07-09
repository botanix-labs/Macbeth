use crate::{
    models::{ChunkId, Snapshot, SnapshotChunk, SnapshotId, SnapshotSync, SnapshotSyncId},
    provider::{database::provider::BotanixDatabaseProvider, SnapshotReader, SnapshotWriter},
    tables,
};
use itertools::Itertools;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{BlockNumber, B256};
use reth_storage_errors::provider::ProviderResult;
use std::{collections::HashMap, ops::RangeInclusive};

impl<TX: DbTx> SnapshotReader for BotanixDatabaseProvider<TX> {
    fn get_snapshots(&self) -> ProviderResult<Vec<Snapshot>> {
        Ok(self
            .tx
            .cursor_read::<tables::Snapshots>()?
            .walk(None)?
            .collect::<Result<HashMap<_, _>, _>>()?
            .values()
            .cloned()
            .sorted_by_key(|s| s.height())
            .collect::<Vec<_>>())
    }

    fn get_snapshot_by_id(&self, snapshot_id: SnapshotId) -> ProviderResult<Option<Snapshot>> {
        Ok(self.tx.get::<tables::Snapshots>(snapshot_id)?)
    }

    fn get_last_snapshot_sync_id(&self) -> ProviderResult<Option<SnapshotSyncId>> {
        Ok(self
            .tx
            .cursor_read::<tables::SnapshotSyncs>()?
            .last()?
            .map(|(snapshot_sync_id, _snapshot_sync)| snapshot_sync_id))
    }

    fn get_snapshot_sync_by_height(&self, height: u64) -> ProviderResult<Option<SnapshotSync>> {
        Ok(self
            .tx
            .cursor_read::<tables::SnapshotSyncs>()?
            .walk(None)?
            .collect::<Result<HashMap<_, _>, _>>()?
            .values()
            .find(|&value| value.height() == height)
            .cloned())
    }

    fn get_snapshot_sync_by_id(&self, id: SnapshotSyncId) -> ProviderResult<Option<SnapshotSync>> {
        Ok(self
            .tx
            .cursor_read::<tables::SnapshotSyncs>()?
            .seek_exact(id)
            .ok()
            .flatten()
            .map(|(_, snapshot_sync)| snapshot_sync))
    }

    fn get_chunk_by_id(&self, chunk_id: ChunkId) -> ProviderResult<Option<SnapshotChunk>> {
        Ok(self.tx.get::<tables::Chunks>(chunk_id)?)
    }

    fn get_chunk_size(&self, chunk_id: ChunkId) -> ProviderResult<usize> {
        Ok(self
            .tx
            .cursor_read::<tables::Chunks>()?
            .seek_exact(chunk_id)
            .ok()
            .flatten()
            .map(|(_, chunk)| chunk.size())
            .unwrap_or_default())
    }

    fn get_snapshot_id_by_block_id(
        &self,
        block_id: BlockNumber,
    ) -> ProviderResult<Option<SnapshotId>> {
        Ok(self.tx.get::<tables::BlockSnapshots>(block_id)?)
    }

    fn get_chunk_block_number(&self, chunk_id: ChunkId) -> ProviderResult<Option<BlockNumber>> {
        Ok(self.tx.get::<tables::ChunkBlocks>(chunk_id)?)
    }

    fn get_last_snapshot_height(&self) -> ProviderResult<Option<(SnapshotId, BlockNumber)>> {
        Ok(self
            .tx
            .cursor_read::<tables::Snapshots>()?
            .last()?
            .map(|(snapshot_id, snapshot)| (snapshot_id, snapshot.height())))
    }

    fn get_first_snapshot_height(&self) -> ProviderResult<Option<(SnapshotId, BlockNumber)>> {
        Ok(self
            .tx
            .cursor_read::<tables::Snapshots>()?
            .first()?
            .map(|(snapshot_id, snapshot)| (snapshot_id, snapshot.height())))
    }

    fn get_snapshot_size(&self, snapshot_id: SnapshotId) -> ProviderResult<usize> {
        let (snapshot_size, chunk_ids) = self
            .tx
            .cursor_read::<tables::Snapshots>()?
            .seek_exact(snapshot_id)
            .ok()
            .flatten()
            .map(|(_, snapshot)| (snapshot.size(), snapshot.chunk_ids().to_vec()))
            .unwrap_or_default();

        let chunks_size = if chunk_ids.is_empty() {
            0
        } else {
            self.tx
                .cursor_read::<tables::Chunks>()?
                .walk_range(
                    chunk_ids.first().cloned().unwrap_or_default()..=
                        chunk_ids.last().cloned().unwrap_or_default(),
                )?
                .collect::<Result<HashMap<_, _>, _>>()?
                .values()
                .map(|value| value.size())
                .sum()
        };

        Ok(snapshot_size + chunks_size)
    }

    fn get_snapshots_count(&self) -> ProviderResult<usize> {
        Ok(self.tx.cursor_read::<tables::Snapshots>()?.walk(None)?.count())
    }

    fn get_last_chunk_id(&self) -> ProviderResult<Option<ChunkId>> {
        Ok(self.tx.cursor_read::<tables::Chunks>()?.last()?.map(|(chunk_id, _chunk)| chunk_id))
    }

    fn get_first_chunk_id(&self) -> ProviderResult<Option<ChunkId>> {
        Ok(self.tx.cursor_read::<tables::Chunks>()?.first()?.map(|(chunk_id, _chunk)| chunk_id))
    }
}

impl<TX: DbTxMut + DbTx> SnapshotWriter for BotanixDatabaseProvider<TX> {
    fn create_new_snapshot_sync(
        &self,
        height: u64,
        snapshot_hash: B256,
        total_chunks: u64,
        format: u64,
    ) -> ProviderResult<SnapshotSyncId> {
        let last_snapshot_sync_id = self.get_last_snapshot_sync_id()?;
        let new_snapshot_sync_id = last_snapshot_sync_id.unwrap_or_default() + 1;
        let new_snapshot_sync = SnapshotSync::new(height, snapshot_hash, format, total_chunks);
        self.tx.put::<tables::SnapshotSyncs>(new_snapshot_sync_id, new_snapshot_sync)?;
        Ok(new_snapshot_sync_id)
    }

    fn create_new_snapshot(
        &self,
        block_number: BlockNumber,
        block_hash: B256,
    ) -> ProviderResult<SnapshotId> {
        let last_snasphot_id =
            self.get_last_snapshot_height()?.map(|snapshot| snapshot.0).unwrap_or(0);
        let new_snapshot_id = last_snasphot_id + 1;
        let mut new_snapshot = Snapshot::default();
        new_snapshot.set_id(new_snapshot_id);
        new_snapshot.set_height(block_number);
        new_snapshot.set_block_hash(block_hash);
        self.tx.put::<tables::Snapshots>(new_snapshot_id, new_snapshot)?;
        self.tx.put::<tables::BlockSnapshots>(block_number, new_snapshot_id)?;
        Ok(new_snapshot_id)
    }

    fn create_new_chunk(
        &self,
        snapshot_id: SnapshotId,
        block_number: BlockNumber,
        chunk_data: Vec<u8>,
    ) -> ProviderResult<ChunkId> {
        let last_chunk_id = self.get_last_chunk_id()?;
        let new_chunk_id = last_chunk_id.unwrap_or_default() + 1;
        let new_chunk = SnapshotChunk::new(snapshot_id, block_number, chunk_data);
        self.tx.put::<tables::Chunks>(new_chunk_id, new_chunk)?;
        self.tx.put::<tables::ChunkBlocks>(new_chunk_id, block_number)?;
        Ok(new_chunk_id)
    }

    fn append_to_chunk(
        &self,
        chunk_id: ChunkId,
        block_number: BlockNumber,
        data: Vec<u8>,
    ) -> ProviderResult<()> {
        let mut chunk = self.get_chunk_by_id(chunk_id)?.expect("chunk exists");
        chunk.append_chunk_data(data, block_number);
        self.tx.put::<tables::Chunks>(chunk_id, chunk)?;
        Ok(())
    }

    fn update_snapshot(
        &self,
        snapshot_id: SnapshotId,
        block_number: BlockNumber,
        chunk_id: ChunkId,
    ) -> ProviderResult<()> {
        let mut plain_cursor = self.tx.cursor_write::<tables::Snapshots>()?;
        let existing_entry = plain_cursor.seek_exact(snapshot_id)?;
        if let Some((snapshot_id, mut snapshot)) = existing_entry {
            snapshot.add_block_id_if_not_exists(block_number);
            snapshot.add_chunk_id_if_not_exists(chunk_id);
            snapshot.set_height(block_number);
            plain_cursor.upsert(snapshot_id, snapshot)?;
        }
        Ok(())
    }

    fn update_snapshot_sync(
        &self,
        snapshot_sync_id: SnapshotSyncId,
        updated_snapshot: SnapshotSync,
    ) -> ProviderResult<()> {
        let mut plain_cursor = self.tx.cursor_write::<tables::SnapshotSyncs>()?;
        plain_cursor.upsert(snapshot_sync_id, updated_snapshot)?;
        Ok(())
    }

    fn remove_block_snapshot_id_mapping(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        self.remove::<tables::BlockSnapshots>(*range.start()..=*range.end())?;
        Ok(())
    }

    fn insert_block_snapshot_id_mapping(
        &self,
        block_id: BlockNumber,
        snapshot_id: SnapshotId,
    ) -> ProviderResult<()> {
        Ok(self.tx.put::<tables::BlockSnapshots>(block_id, snapshot_id)?)
    }

    fn remove_snapshots(&self, range: RangeInclusive<SnapshotId>) -> ProviderResult<()> {
        if range.is_empty() {
            return Ok(())
        }
        self.remove::<tables::Snapshots>(*range.start()..=*range.end())?;
        Ok(())
    }

    fn remove_oldest_snapshot(&self) -> ProviderResult<()> {
        if let Some((snapshot_id, _)) = self.get_first_snapshot_height()? {
            let snapshot = self.get_snapshot_by_id(snapshot_id)?.expect("Snapshot exists");
            self.remove_snapshots(RangeInclusive::new(snapshot_id, snapshot_id))?;
            let cids = snapshot.chunk_ids().to_vec();
            if cids.is_empty() {
                return Ok(())
            }
            let range_to_delete = RangeInclusive::new(
                cids.first().copied().unwrap_or_default(),
                cids.last().copied().unwrap_or_default(),
            );
            self.remove_chunks(range_to_delete.clone())?;
            self.delete_chunks_in_blocks(range_to_delete)?;
        }
        Ok(())
    }

    fn remove_chunks(&self, range: RangeInclusive<ChunkId>) -> ProviderResult<()> {
        if range.is_empty() {
            return Ok(())
        }
        self.remove::<tables::Chunks>(*range.start()..=*range.end())?;
        Ok(())
    }

    fn delete_chunks_in_blocks(&self, range: RangeInclusive<ChunkId>) -> ProviderResult<()> {
        if range.is_empty() {
            return Ok(())
        }
        Ok(self.tx.cursor_write::<tables::ChunkBlocks>()?.walk_range(range)?.delete_current()?)
    }
}
