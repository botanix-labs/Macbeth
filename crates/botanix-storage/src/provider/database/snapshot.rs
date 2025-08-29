use crate::{
    models::{ChunkId, Snapshot, SnapshotChunk, SnapshotId, SnapshotSync, SnapshotSyncId},
    provider::{database::provider::BotanixDatabaseProvider, SnapshotReader, SnapshotWriter},
    tables::{BlockSnapshots, ChunkBlocks, Chunks, SnapshotSyncs, Snapshots},
    BotanixDatabaseProviderRW,
};
use itertools::Itertools;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_primitives::{BlockNumber, B256};
use reth_storage_errors::provider::ProviderResult;
use std::{collections::HashMap, ops::RangeInclusive};

impl<TX: DbTx> SnapshotReader for BotanixDatabaseProvider<TX> {
    fn get_snapshots(&self) -> ProviderResult<Vec<Snapshot>> {
        Ok(self
            .tx
            .cursor_read::<Snapshots>()?
            .walk(None)?
            .collect::<Result<HashMap<_, _>, _>>()?
            .values()
            .cloned()
            .sorted_by_key(|s| s.height())
            .collect::<Vec<_>>())
    }

    fn get_snapshot_by_id(&self, snapshot_id: SnapshotId) -> ProviderResult<Option<Snapshot>> {
        Ok(self.tx.get::<Snapshots>(snapshot_id)?)
    }

    fn get_last_snapshot_sync_id(&self) -> ProviderResult<Option<SnapshotSyncId>> {
        Ok(self
            .tx
            .cursor_read::<SnapshotSyncs>()?
            .last()?
            .map(|(snapshot_sync_id, _snapshot_sync)| snapshot_sync_id))
    }

    fn get_snapshot_sync_by_height(&self, height: u64) -> ProviderResult<Option<SnapshotSync>> {
        Ok(self
            .tx
            .cursor_read::<SnapshotSyncs>()?
            .walk(None)?
            .collect::<Result<HashMap<_, _>, _>>()?
            .values()
            .find(|&value| value.height() == height)
            .cloned())
    }

    fn get_snapshot_sync_by_id(&self, id: SnapshotSyncId) -> ProviderResult<Option<SnapshotSync>> {
        Ok(self
            .tx
            .cursor_read::<SnapshotSyncs>()?
            .seek_exact(id)
            .ok()
            .flatten()
            .map(|(_, snapshot_sync)| snapshot_sync))
    }

    fn get_chunk_by_id(&self, chunk_id: ChunkId) -> ProviderResult<Option<SnapshotChunk>> {
        Ok(self.tx.get::<Chunks>(chunk_id)?)
    }

    fn get_chunk_size(&self, chunk_id: ChunkId) -> ProviderResult<usize> {
        Ok(self
            .tx
            .cursor_read::<Chunks>()?
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
        Ok(self.tx.get::<BlockSnapshots>(block_id)?)
    }

    fn get_chunk_block_number(&self, chunk_id: ChunkId) -> ProviderResult<Option<BlockNumber>> {
        Ok(self.tx.get::<ChunkBlocks>(chunk_id)?)
    }

    fn get_last_snapshot_height(&self) -> ProviderResult<Option<(SnapshotId, BlockNumber)>> {
        Ok(self
            .tx
            .cursor_read::<Snapshots>()?
            .last()?
            .map(|(snapshot_id, snapshot)| (snapshot_id, snapshot.height())))
    }

    fn get_first_snapshot_height(&self) -> ProviderResult<Option<(SnapshotId, BlockNumber)>> {
        Ok(self
            .tx
            .cursor_read::<Snapshots>()?
            .first()?
            .map(|(snapshot_id, snapshot)| (snapshot_id, snapshot.height())))
    }

    fn get_snapshot_size(&self, snapshot_id: SnapshotId) -> ProviderResult<usize> {
        let (snapshot_size, chunk_ids) = self
            .tx
            .cursor_read::<Snapshots>()?
            .seek_exact(snapshot_id)
            .ok()
            .flatten()
            .map(|(_, snapshot)| (snapshot.size(), snapshot.chunk_ids().to_vec()))
            .unwrap_or_default();

        let chunks_size = if chunk_ids.is_empty() {
            0
        } else {
            self.tx
                .cursor_read::<Chunks>()?
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
        Ok(self.tx.cursor_read::<Snapshots>()?.walk(None)?.count())
    }

    fn get_last_chunk_id(&self) -> ProviderResult<Option<ChunkId>> {
        Ok(self.tx.cursor_read::<Chunks>()?.last()?.map(|(chunk_id, _chunk)| chunk_id))
    }

    fn get_first_chunk_id(&self) -> ProviderResult<Option<ChunkId>> {
        Ok(self.tx.cursor_read::<Chunks>()?.first()?.map(|(chunk_id, _chunk)| chunk_id))
    }
}

impl<DB: Database> SnapshotReader for BotanixDatabaseProviderRW<DB> {
    #[inline(always)]
    fn get_snapshots(&self) -> ProviderResult<Vec<Snapshot>> {
        self.0.get_snapshots()
    }

    #[inline(always)]
    fn get_snapshot_by_id(&self, snapshot_id: SnapshotId) -> ProviderResult<Option<Snapshot>> {
        self.0.get_snapshot_by_id(snapshot_id)
    }

    #[inline(always)]
    fn get_last_snapshot_sync_id(&self) -> ProviderResult<Option<SnapshotSyncId>> {
        self.0.get_last_snapshot_sync_id()
    }

    #[inline(always)]
    fn get_snapshot_sync_by_height(&self, height: u64) -> ProviderResult<Option<SnapshotSync>> {
        self.0.get_snapshot_sync_by_height(height)
    }

    #[inline(always)]
    fn get_snapshot_sync_by_id(&self, id: u64) -> ProviderResult<Option<SnapshotSync>> {
        self.0.get_snapshot_sync_by_id(id)
    }

    #[inline(always)]
    fn get_chunk_by_id(&self, chunk_id: ChunkId) -> ProviderResult<Option<SnapshotChunk>> {
        self.0.get_chunk_by_id(chunk_id)
    }

    #[inline(always)]
    fn get_chunk_size(&self, chunk_id: ChunkId) -> ProviderResult<usize> {
        self.0.get_chunk_size(chunk_id)
    }

    #[inline(always)]
    fn get_snapshot_id_by_block_id(
        &self,
        block_id: BlockNumber,
    ) -> ProviderResult<Option<SnapshotId>> {
        self.0.get_snapshot_id_by_block_id(block_id)
    }

    #[inline(always)]
    fn get_chunk_block_number(&self, chunk_id: ChunkId) -> ProviderResult<Option<BlockNumber>> {
        self.0.get_chunk_block_number(chunk_id)
    }

    #[inline(always)]
    fn get_last_snapshot_height(&self) -> ProviderResult<Option<(SnapshotId, BlockNumber)>> {
        self.0.get_last_snapshot_height()
    }

    #[inline(always)]
    fn get_first_snapshot_height(&self) -> ProviderResult<Option<(SnapshotId, BlockNumber)>> {
        self.0.get_first_snapshot_height()
    }

    #[inline(always)]
    fn get_snapshot_size(&self, snapshot_id: SnapshotId) -> ProviderResult<usize> {
        self.0.get_snapshot_size(snapshot_id)
    }

    #[inline(always)]
    fn get_snapshots_count(&self) -> ProviderResult<usize> {
        self.0.get_snapshots_count()
    }

    #[inline(always)]
    fn get_last_chunk_id(&self) -> ProviderResult<Option<ChunkId>> {
        self.0.get_last_chunk_id()
    }

    #[inline(always)]
    fn get_first_chunk_id(&self) -> ProviderResult<Option<ChunkId>> {
        self.0.get_first_chunk_id()
    }
}

impl<DB: Database> SnapshotWriter for BotanixDatabaseProviderRW<DB> {
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
        self.tx.put::<SnapshotSyncs>(new_snapshot_sync_id, new_snapshot_sync)?;
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
        self.tx.put::<Snapshots>(new_snapshot_id, new_snapshot)?;
        self.tx.put::<BlockSnapshots>(block_number, new_snapshot_id)?;
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
        self.tx.put::<Chunks>(new_chunk_id, new_chunk)?;
        self.tx.put::<ChunkBlocks>(new_chunk_id, block_number)?;
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
        self.tx.put::<Chunks>(chunk_id, chunk)?;
        Ok(())
    }

    fn update_snapshot(
        &self,
        snapshot_id: SnapshotId,
        block_number: BlockNumber,
        chunk_id: ChunkId,
    ) -> ProviderResult<()> {
        let mut plain_cursor = self.tx.cursor_write::<Snapshots>()?;
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
        let mut plain_cursor = self.tx.cursor_write::<SnapshotSyncs>()?;
        plain_cursor.upsert(snapshot_sync_id, updated_snapshot)?;
        Ok(())
    }

    fn remove_block_snapshot_id_mapping(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        self.remove::<BlockSnapshots>(*range.start()..=*range.end())?;
        Ok(())
    }

    fn insert_block_snapshot_id_mapping(
        &self,
        block_id: BlockNumber,
        snapshot_id: SnapshotId,
    ) -> ProviderResult<()> {
        Ok(self.tx.put::<BlockSnapshots>(block_id, snapshot_id)?)
    }

    fn remove_snapshots(&self, range: RangeInclusive<SnapshotId>) -> ProviderResult<()> {
        if range.is_empty() {
            return Ok(())
        }
        self.remove::<Snapshots>(*range.start()..=*range.end())?;
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

        self.remove::<Chunks>(*range.start()..=*range.end())?;

        Ok(())
    }

    fn delete_chunks_in_blocks(&self, range: RangeInclusive<ChunkId>) -> ProviderResult<()> {
        if range.is_empty() {
            return Ok(())
        }

        self.remove::<ChunkBlocks>(*range.start()..=*range.end())?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::create_test_provider_factory, DatabaseProviderFactoryRO,
        DatabaseProviderFactoryRW,
    };
    use reth_primitives::B256;

    #[test]
    fn test_get_snapshots_empty_database() {
        let factory = create_test_provider_factory();
        let provider = factory.provider().expect("Failed to create provider");

        let snapshots = provider.get_snapshots().expect("Failed to get snapshots");

        assert!(snapshots.is_empty());
    }

    #[test]
    fn test_get_snapshots_sorted_by_height() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        let block_hash = B256::random();
        let snapshot_id_1 =
            provider.create_new_snapshot(100, block_hash).expect("Failed to create snapshot 1");
        let snapshot_id_2 =
            provider.create_new_snapshot(50, block_hash).expect("Failed to create snapshot 2");
        let snapshot_id_3 =
            provider.create_new_snapshot(200, block_hash).expect("Failed to create snapshot 3");
        provider.commit().expect("Failed to commit transaction");

        let provider = factory.provider().expect("Failed to create RO provider");
        let snapshots = provider.get_snapshots().expect("Failed to get snapshots");

        assert_eq!(snapshots.len(), 3);
        assert_eq!(snapshots[0].height(), 50);
        assert_eq!(snapshots[1].height(), 100);
        assert_eq!(snapshots[2].height(), 200);
        assert_eq!(snapshots[0].id(), snapshot_id_2);
        assert_eq!(snapshots[1].id(), snapshot_id_1);
        assert_eq!(snapshots[2].id(), snapshot_id_3);
    }

    #[test]
    fn test_get_snapshot_sync_by_height_with_multiple_syncs() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        let hash1 = B256::random();
        let hash2 = B256::random();
        let hash3 = B256::random();

        provider
            .create_new_snapshot_sync(100, hash1, 5, 1)
            .expect("Failed to create snapshot sync 1");
        provider
            .create_new_snapshot_sync(200, hash2, 10, 1)
            .expect("Failed to create snapshot sync 2");
        provider
            .create_new_snapshot_sync(150, hash3, 7, 1)
            .expect("Failed to create snapshot sync 3");

        provider.commit().expect("Failed to commit transaction");

        let provider = factory.provider().expect("Failed to create RO provider");

        let sync_at_100 =
            provider.get_snapshot_sync_by_height(100).expect("Failed to get sync at height 100");

        assert!(sync_at_100.is_some());

        let sync_100 = sync_at_100.expect("Expected sync to exist at height 100");

        assert_eq!(sync_100.height(), 100);
        assert_eq!(sync_100.snapshot_hash(), hash1);

        let sync_at_200 =
            provider.get_snapshot_sync_by_height(200).expect("Failed to get sync at height 200");

        assert!(sync_at_200.is_some());

        let sync_200 = sync_at_200.expect("Expected sync to exist at height 200");

        assert_eq!(sync_200.height(), 200);
        assert_eq!(sync_200.snapshot_hash(), hash2);

        let sync_at_999 =
            provider.get_snapshot_sync_by_height(999).expect("Failed to get sync at height 999");

        assert!(sync_at_999.is_none());
    }

    #[test]
    fn test_get_chunk_size_existing_chunk() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        let block_hash = B256::random();
        let snapshot_id = provider
            .create_new_snapshot(100, block_hash)
            .expect("Failed to create snapshot at height 100");

        let data = vec![1, 2, 3, 4, 5];
        let chunk_id = provider
            .create_new_chunk(snapshot_id, 100, data.clone())
            .expect("Failed to create chunk for snapshot");

        provider.commit().expect("Failed to commit transaction");

        let provider = factory.provider().expect("Failed to create RO provider");
        let size = provider.get_chunk_size(chunk_id).expect("Failed to get chunk size");

        let expected_size = std::mem::size_of::<u64>() + data.len();
        assert_eq!(size, expected_size);
    }

    #[test]
    fn test_get_chunk_size_nonexistent_chunk() {
        let factory = create_test_provider_factory();
        let provider = factory.provider().expect("Failed to create RO provider");

        let size = provider.get_chunk_size(999).expect("Failed to get size of nonexistent chunk");
        assert_eq!(size, 0);
    }

    #[test]
    fn test_get_snapshot_size_with_chunks() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        let block_hash = B256::random();
        let snapshot_id = provider
            .create_new_snapshot(100, block_hash)
            .expect("Failed to create snapshot at height 100");

        let data1 = vec![1, 2, 3];
        let data2 = vec![4, 5, 6, 7, 8];

        let chunk_id_1 = provider
            .create_new_chunk(snapshot_id, 100, data1.clone())
            .expect("Failed to create first chunk");
        let chunk_id_2 = provider
            .create_new_chunk(snapshot_id, 101, data2.clone())
            .expect("Failed to create second chunk");

        provider
            .update_snapshot(snapshot_id, 100, chunk_id_1)
            .expect("Failed to update snapshot with first chunk");
        provider
            .update_snapshot(snapshot_id, 101, chunk_id_2)
            .expect("Failed to update snapshot with second chunk");

        provider.commit().expect("Failed to commit transaction");

        let provider = factory.provider().expect("Failed to create RO provider");
        let snapshot = provider
            .get_snapshot_by_id(snapshot_id)
            .expect("Failed to get snapshot by ID")
            .expect("Expected snapshot to exist");
        let total_size = provider
            .get_snapshot_size(snapshot_id)
            .expect("Failed to calculate total snapshot size");

        let expected_snapshot_size = snapshot.size();
        let expected_chunk_1_size = std::mem::size_of::<u64>() + data1.len();
        let expected_chunk_2_size = std::mem::size_of::<u64>() + data2.len();
        let expected_total_size =
            expected_snapshot_size + expected_chunk_1_size + expected_chunk_2_size;

        assert_eq!(total_size, expected_total_size);
    }

    #[test]
    fn test_get_snapshot_size_no_chunks() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        let block_hash = B256::random();
        let snapshot_id = provider
            .create_new_snapshot(100, block_hash)
            .expect("Failed to create snapshot at height 100");
        provider.commit().expect("Failed to commit transaction");

        let provider = factory.provider().expect("Failed to create RO provider");
        let snapshot = provider
            .get_snapshot_by_id(snapshot_id)
            .expect("Failed to get snapshot by ID")
            .expect("Expected snapshot to exist");
        let total_size =
            provider.get_snapshot_size(snapshot_id).expect("Failed to calculate snapshot size");

        assert_eq!(total_size, snapshot.size());
    }

    #[test]
    fn test_get_snapshot_size_nonexistent_snapshot() {
        let factory = create_test_provider_factory();
        let provider = factory.provider().expect("Failed to create RO provider");

        let size =
            provider.get_snapshot_size(999).expect("Failed to get size of nonexistent snapshot");
        assert_eq!(size, 0);
    }

    #[test]
    fn test_create_new_snapshot_sync_id_generation() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        let hash1 = B256::random();
        let hash2 = B256::random();

        let sync_id_1 = provider
            .create_new_snapshot_sync(100, hash1, 5, 1)
            .expect("Failed to create first snapshot sync");
        let sync_id_2 = provider
            .create_new_snapshot_sync(200, hash2, 10, 1)
            .expect("Failed to create second snapshot sync");

        provider.commit().expect("Failed to commit transaction");

        assert_eq!(sync_id_1, 1);
        assert_eq!(sync_id_2, 2);
    }

    #[test]
    fn test_create_new_snapshot_sync_first_sync() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        let hash = B256::random();
        let sync_id = provider
            .create_new_snapshot_sync(100, hash, 5, 1)
            .expect("Failed to create snapshot sync");

        provider.commit().expect("Failed to commit transaction");

        let provider = factory.provider().expect("Failed to create RO provider");

        let sync = provider
            .get_snapshot_sync_by_id(sync_id)
            .expect("Failed to get snapshot by ID")
            .expect("Expected snapshot to exist");

        assert_eq!(sync_id, 1);
        assert_eq!(sync.height(), 100);
        assert_eq!(sync.snapshot_hash(), hash);
        assert_eq!(sync.total_chunks(), 5);
        assert_eq!(sync.format(), 1);
    }

    #[test]
    fn test_create_new_snapshot_with_id_generation() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        let block_hash_1 = B256::random();
        let block_hash_2 = B256::random();

        let snapshot_id_1 = provider
            .create_new_snapshot(100, block_hash_1)
            .expect("Failed to create first snapshot");
        let snapshot_id_2 = provider
            .create_new_snapshot(200, block_hash_2)
            .expect("Failed to create second snapshot");

        provider.commit().expect("Failed to commit transaction");

        assert_eq!(snapshot_id_1, 1);
        assert_eq!(snapshot_id_2, 2);

        let provider = factory.provider().expect("Failed to create RO provider");
        let snapshot_1 = provider
            .get_snapshot_by_id(snapshot_id_1)
            .expect("Failed to get snapshot by ID")
            .expect("Expected snapshot to exist");

        let snapshot_2 = provider
            .get_snapshot_by_id(snapshot_id_2)
            .expect("Failed to get snapshot by ID")
            .expect("Expected snapshot to exist");

        assert_eq!(snapshot_1.height(), 100);
        assert_eq!(snapshot_1.block_hash(), block_hash_1);
        assert_eq!(snapshot_2.height(), 200);
        assert_eq!(snapshot_2.block_hash(), block_hash_2);

        let block_snapshot_1 = provider
            .get_snapshot_id_by_block_id(100)
            .expect("Failed to get snapshot by ID")
            .expect("Expected snapshot to exist");

        let block_snapshot_2 = provider
            .get_snapshot_id_by_block_id(200)
            .expect("Failed to get snapshot by ID")
            .expect("Expected snapshot to exist");

        assert_eq!(block_snapshot_1, snapshot_id_1);
        assert_eq!(block_snapshot_2, snapshot_id_2);
    }

    #[test]
    fn test_update_snapshot_adds_new_data() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        let block_hash = B256::random();
        let snapshot_id = provider
            .create_new_snapshot(100, block_hash)
            .expect("Failed to create snapshot at height 100");

        let data = vec![1, 2, 3];
        let chunk_id = provider
            .create_new_chunk(snapshot_id, 101, data)
            .expect("Failed to create chunk for snapshot");

        provider
            .update_snapshot(snapshot_id, 101, chunk_id)
            .expect("Failed to update snapshot with chunk");
        provider.commit().expect("Failed to commit transaction");

        let provider = factory.provider().expect("Failed to create RO provider");
        let snapshot = provider
            .get_snapshot_by_id(snapshot_id)
            .expect("Failed to get snapshot by ID")
            .expect("Expected snapshot to exist");

        assert_eq!(snapshot.height(), 101);
        assert!(snapshot.chunk_ids().contains(&chunk_id));
        assert!(snapshot.block_ids().contains(&101));
    }

    #[test]
    fn test_update_snapshot_handles_duplicates() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        let block_hash = B256::random();
        let snapshot_id = provider
            .create_new_snapshot(100, block_hash)
            .expect("Failed to create snapshot at height 100");

        let data = vec![1, 2, 3];
        let chunk_id = provider
            .create_new_chunk(snapshot_id, 101, data)
            .expect("Failed to create chunk for snapshot");

        provider
            .update_snapshot(snapshot_id, 101, chunk_id)
            .expect("Failed to update snapshot with chunk");
        provider
            .update_snapshot(snapshot_id, 101, chunk_id)
            .expect("Failed to update snapshot with chunk");
        provider.commit().expect("Failed to commit transaction");

        let provider = factory.provider().expect("Failed to create RO provider");
        let snapshot = provider
            .get_snapshot_by_id(snapshot_id)
            .expect("Failed to get snapshot by ID")
            .expect("Expected snapshot to exist");

        assert_eq!(snapshot.chunk_ids().len(), 1);
        assert_eq!(snapshot.block_ids().len(), 1);
        assert_eq!(snapshot.chunk_ids()[0], chunk_id);
        assert_eq!(snapshot.block_ids()[0], 101);
    }

    #[test]
    fn test_update_snapshot_nonexistent_snapshot() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        provider.update_snapshot(999, 100, 1).expect("Failed to update nonexistent snapshot");
        provider.commit().expect("Failed to commit transaction");

        let provider = factory.provider().expect("Failed to create RO provider");
        let snapshot =
            provider.get_snapshot_by_id(999).expect("Failed to get nonexistent snapshot");
        assert!(snapshot.is_none());
    }

    #[test]
    fn test_remove_oldest_snapshot_with_chunks() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        let block_hash = B256::random();
        let snapshot_id_1 = provider
            .create_new_snapshot(100, block_hash)
            .expect("Failed to create snapshot at height 100");
        let snapshot_id_2 = provider
            .create_new_snapshot(200, block_hash)
            .expect("Failed to create second snapshot at height 200");

        let data = vec![1, 2, 3, 4, 5];
        let chunk_id_1 = provider
            .create_new_chunk(snapshot_id_1, 100, data.clone())
            .expect("Failed to create chunk for first snapshot");
        let chunk_id_2 = provider
            .create_new_chunk(snapshot_id_2, 200, data)
            .expect("Failed to create chunk for second snapshot");

        provider
            .update_snapshot(snapshot_id_1, 100, chunk_id_1)
            .expect("Failed to update first snapshot");
        provider
            .update_snapshot(snapshot_id_2, 200, chunk_id_2)
            .expect("Failed to update second snapshot");

        provider.remove_oldest_snapshot().expect("Failed to remove oldest snapshot");
        provider.commit().expect("Failed to commit transaction");

        let provider = factory.provider().expect("Failed to create RO provider");

        let remaining_snapshot =
            provider.get_snapshot_by_id(snapshot_id_1).expect("Failed to get first snapshot");
        assert!(remaining_snapshot.is_none());

        let existing_snapshot =
            provider.get_snapshot_by_id(snapshot_id_2).expect("Failed to get second snapshot");
        assert!(existing_snapshot.is_some());

        let removed_chunk =
            provider.get_chunk_by_id(chunk_id_1).expect("Failed to get first chunk");
        assert!(removed_chunk.is_none());

        let existing_chunk =
            provider.get_chunk_by_id(chunk_id_2).expect("Failed to get second chunk");
        assert!(existing_chunk.is_some());

        let removed_chunk_block = provider
            .get_chunk_block_number(chunk_id_1)
            .expect("Failed to get block number for first chunk");
        assert!(removed_chunk_block.is_none());

        let existing_chunk_block = provider
            .get_chunk_block_number(chunk_id_2)
            .expect("Failed to get block number for second chunk");
        assert!(existing_chunk_block.is_some());
    }

    #[test]
    fn test_remove_oldest_snapshot_no_chunks() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        let block_hash = B256::random();
        let snapshot_id = provider
            .create_new_snapshot(100, block_hash)
            .expect("Failed to create snapshot at height 100");

        provider.remove_oldest_snapshot().expect("Failed to remove oldest snapshot");
        provider.commit().expect("Failed to commit transaction");

        let provider = factory.provider().expect("Failed to create RO provider");
        let snapshot = provider.get_snapshot_by_id(snapshot_id).expect("Failed get snapshot by id");
        assert!(snapshot.is_none());
    }

    #[test]
    fn test_remove_oldest_snapshot_empty_database() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        provider.remove_oldest_snapshot().expect("Failed to remove oldest snapshot");
        provider.commit().expect("Failed to commit transaction");
    }

    #[test]
    fn test_remove_oldest_snapshot_complex_chunk_range() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().expect("Failed to create RW provider");

        let block_hash = B256::random();
        let snapshot_id = provider
            .create_new_snapshot(100, block_hash)
            .expect("Failed to create snapshot at height 100");

        let data = vec![1, 2, 3];
        let chunk_id_1 = provider
            .create_new_chunk(snapshot_id, 100, data.clone())
            .expect("Failed to create chunk at block 100");
        let chunk_id_2 = provider
            .create_new_chunk(snapshot_id, 101, data.clone())
            .expect("Failed to create chunk at block 101");
        let chunk_id_3 = provider
            .create_new_chunk(snapshot_id, 102, data)
            .expect("Failed to create chunk at block 102");

        provider
            .update_snapshot(snapshot_id, 100, chunk_id_1)
            .expect("Failed to update snapshot with first chunk");
        provider
            .update_snapshot(snapshot_id, 101, chunk_id_2)
            .expect("Failed to update snapshot with second chunk");
        provider
            .update_snapshot(snapshot_id, 102, chunk_id_3)
            .expect("Failed to update snapshot with third chunk");

        provider.remove_oldest_snapshot().expect("Failed to remove oldest snapshot");
        provider.commit().expect("Failed to commit transaction");

        let provider = factory.provider().expect("Failed to create RO provider");

        let snapshot =
            provider.get_snapshot_by_id(snapshot_id).expect("Failed to get snapshot by id");

        assert!(snapshot.is_none());

        assert!(provider.get_chunk_by_id(chunk_id_1).expect("Failed to get first chunk").is_none());

        assert!(provider
            .get_chunk_by_id(chunk_id_2)
            .expect("Failed to get second chunk")
            .is_none());

        assert!(provider.get_chunk_by_id(chunk_id_3).expect("Failed to get chunk by id").is_none());

        assert!(provider
            .get_chunk_block_number(chunk_id_1)
            .expect("Failed to get block number for first chunk")
            .is_none());

        assert!(provider
            .get_chunk_block_number(chunk_id_2)
            .expect("Failed to get block number for second chunk")
            .is_none());

        assert!(provider
            .get_chunk_block_number(chunk_id_3)
            .expect("Failed to get block number for third chunk")
            .is_none());
    }

    #[test]
    fn test_get_last_chunk_id_bug_fix() {
        let factory = create_test_provider_factory();
        let provider_rw = factory.provider_rw().expect("Failed to create RW provider");

        let block_hash = B256::random();

        let snapshot_id = provider_rw
            .create_new_snapshot(100, block_hash)
            .expect("Failed to create new snapshot");

        let data = vec![1, 2, 3];

        let chunk_id = provider_rw
            .create_new_chunk(snapshot_id, 100, data)
            .expect("Failed to create new chunk");

        provider_rw.commit().expect("Failed to commit transaction");

        let provider_rw = factory.provider_rw().expect("Failed to create RW provider");
        let last_chunk_id = provider_rw.get_last_chunk_id().expect("Failed to get last chunk id");

        assert_eq!(last_chunk_id, Some(chunk_id));
    }
}
