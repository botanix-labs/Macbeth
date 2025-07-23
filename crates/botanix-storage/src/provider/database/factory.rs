use crate::{
    models::{
        ChunkId, HeaderWithPegs, PeerID, RuntimeVersion, Snapshot, SnapshotChunk, SnapshotId,
        SnapshotSync, SnapshotSyncId, UuidID, WalletStateSyncRecord,
    },
    provider::database::provider::{
        BotanixDatabaseProvider, BotanixDatabaseProviderRO, BotanixDatabaseProviderRW,
    },
    DatabaseProviderFactoryRO, DatabaseProviderFactoryRW, RuntimeTransitionsReadWrite,
    SnapshotReader, SnapshotWriter, StagedHeaderReader, StagedHeaderWriter, WalletStateSyncReader,
    WalletStateSyncWriter,
};
use reth_db::{init_db, mdbx::DatabaseArguments, DatabaseEnv};
use reth_db_api::database::Database;
use reth_primitives::{BlockNumber, Bytes, B256};
use reth_storage_errors::provider::ProviderResult;
use std::{collections::HashSet, ops::RangeInclusive, path::Path, sync::Arc};

/// A common provider that fetches data from a database or static file.
///
/// This provider factory implements most provider or provider factory traits and serves
/// as the primary entry point for accessing the Botanix storage system. It manages
/// database connections and provides both read-only and read-write access to stored data.
///
/// ## Generic Parameters
///
/// * `DB` - The database type, typically `DatabaseEnv` for production use
///
/// ## Usage
///
/// ```rust,ignore
/// use botanix_storage::BotanixProviderFactory;
/// use reth_db::{init_db, mdbx::DatabaseArguments};
///
/// // Create factory with existing database
/// let factory = BotanixProviderFactory::new(database);
///
/// // Or create with database path
/// let factory = BotanixProviderFactory::new_with_database_path(
///     "./data/botanix.db",
///     DatabaseArguments::default()
/// )?;
///
/// // Use for read operations
/// let provider = factory.provider()?;
/// let snapshots = provider.get_snapshots()?;
///
/// // Use for write operations
/// let provider_rw = factory.provider_rw()?;
/// let snapshot_id = provider_rw.create_new_snapshot(block_number, block_hash)?;
/// provider_rw.commit()?;
/// ```
#[derive(Debug, Clone)]
pub struct BotanixProviderFactory<DB> {
    /// Database instance wrapped in Arc for thread-safe sharing
    db: Arc<DB>,
}

impl<DB> BotanixProviderFactory<DB> {
    /// Create new database provider factory.
    ///
    /// Creates a new factory instance that wraps the provided database.
    /// The database is stored in an `Arc` to allow for efficient cloning
    /// and sharing across multiple threads.
    ///
    /// # Parameters
    ///
    /// * `db` - The database instance to wrap
    ///
    /// # Returns
    ///
    /// A new `BotanixProviderFactory` instance
    pub fn new(db: DB) -> Self {
        Self { db: Arc::new(db) }
    }

    /// Returns reference to the underlying database.
    ///
    /// Provides direct access to the underlying database instance.
    /// This is useful for advanced operations that require direct
    /// database access outside of the provider trait methods.
    ///
    /// # Returns
    ///
    /// A reference to the underlying database instance
    pub fn db_ref(&self) -> &DB {
        self.db.as_ref()
    }

    #[cfg(any(test, feature = "test-utils"))]
    /// Consumes Self and returns DB
    ///
    /// Consumes the factory and returns the underlying database instance.
    /// This is primarily useful for testing scenarios where you need to
    /// access the database directly after using the factory.
    ///
    /// # Returns
    ///
    /// The underlying database instance wrapped in an `Arc`
    pub fn into_db(self) -> Arc<DB> {
        self.db
    }
}

impl BotanixProviderFactory<DatabaseEnv> {
    /// Create new database provider by passing a path. [`BotanixProviderFactory`] will own the
    /// database instance.
    ///
    /// This constructor initializes a new MDBX database at the specified path and
    /// wraps it in a provider factory. The factory takes ownership of the database
    /// instance, ensuring proper lifecycle management.
    ///
    /// # Parameters
    ///
    /// * `path` - The filesystem path where the database should be created or opened
    /// * `args` - Database configuration arguments (page size, map size, etc.)
    ///
    /// # Returns
    ///
    /// * `Ok(BotanixProviderFactory)` - Successfully created factory with database
    /// * `Err(eyre::Error)` - If database initialization failed
    ///
    /// # Errors
    ///
    /// This method will return an error if:
    /// - The specified path is invalid or inaccessible
    /// - Database initialization fails due to permissions or disk space
    /// - The database file is corrupted or incompatible
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use botanix_storage::BotanixProviderFactory;
    /// use reth_db::mdbx::DatabaseArguments;
    ///
    /// let factory = BotanixProviderFactory::new_with_database_path(
    ///     "./data/botanix.db",
    ///     DatabaseArguments::default()
    /// )?;
    /// ```
    pub fn new_with_database_path<P: AsRef<Path>>(
        path: P,
        args: DatabaseArguments,
    ) -> eyre::Result<Self> {
        Ok(Self { db: Arc::new(init_db(path, args)?) })
    }
}

impl<DB> DatabaseProviderFactoryRO for BotanixProviderFactory<DB>
where
    DB: Database,
{
    type Provider = BotanixDatabaseProviderRO<DB>;

    #[track_caller]
    fn provider(&self) -> ProviderResult<Self::Provider> {
        Ok(BotanixDatabaseProvider::new(self.db.tx()?))
    }
}

impl<DB> DatabaseProviderFactoryRW for BotanixProviderFactory<DB>
where
    DB: Database,
{
    type Provider = BotanixDatabaseProviderRW<DB>;

    #[track_caller]
    fn provider_rw(&self) -> ProviderResult<Self::Provider> {
        Ok(BotanixDatabaseProviderRW(BotanixDatabaseProvider::new_rw(self.db.tx_mut()?)))
    }
}

impl<DB: Database> SnapshotReader for BotanixProviderFactory<DB> {
    #[inline(always)]
    fn get_snapshots(&self) -> ProviderResult<Vec<Snapshot>> {
        self.provider()?.get_snapshots()
    }

    #[inline(always)]
    fn get_snapshot_by_id(&self, snapshot_id: SnapshotId) -> ProviderResult<Option<Snapshot>> {
        self.provider()?.get_snapshot_by_id(snapshot_id)
    }

    #[inline(always)]
    fn get_last_snapshot_sync_id(&self) -> ProviderResult<Option<SnapshotSyncId>> {
        self.provider()?.get_last_snapshot_sync_id()
    }

    #[inline(always)]
    fn get_snapshot_sync_by_height(&self, height: u64) -> ProviderResult<Option<SnapshotSync>> {
        self.provider()?.get_snapshot_sync_by_height(height)
    }

    #[inline(always)]
    fn get_snapshot_sync_by_id(&self, id: u64) -> ProviderResult<Option<SnapshotSync>> {
        self.provider()?.get_snapshot_sync_by_id(id)
    }

    #[inline(always)]
    fn get_chunk_by_id(&self, chunk_id: ChunkId) -> ProviderResult<Option<SnapshotChunk>> {
        self.provider()?.get_chunk_by_id(chunk_id)
    }

    #[inline(always)]
    fn get_chunk_size(&self, chunk_id: ChunkId) -> ProviderResult<usize> {
        self.provider()?.get_chunk_size(chunk_id)
    }

    #[inline(always)]
    fn get_snapshot_id_by_block_id(
        &self,
        block_id: BlockNumber,
    ) -> ProviderResult<Option<SnapshotId>> {
        self.provider()?.get_snapshot_id_by_block_id(block_id)
    }

    #[inline(always)]
    fn get_chunk_block_number(&self, chunk_id: ChunkId) -> ProviderResult<Option<BlockNumber>> {
        self.provider()?.get_chunk_block_number(chunk_id)
    }

    #[inline(always)]
    fn get_last_snapshot_height(&self) -> ProviderResult<Option<(SnapshotId, BlockNumber)>> {
        self.provider()?.get_last_snapshot_height()
    }

    #[inline(always)]
    fn get_first_snapshot_height(&self) -> ProviderResult<Option<(SnapshotId, BlockNumber)>> {
        self.provider()?.get_first_snapshot_height()
    }

    #[inline(always)]
    fn get_snapshot_size(&self, snapshot_id: SnapshotId) -> ProviderResult<usize> {
        self.provider()?.get_snapshot_size(snapshot_id)
    }

    #[inline(always)]
    fn get_snapshots_count(&self) -> ProviderResult<usize> {
        self.provider()?.get_snapshots_count()
    }

    #[inline(always)]
    fn get_last_chunk_id(&self) -> ProviderResult<Option<ChunkId>> {
        self.provider()?.get_last_chunk_id()
    }

    #[inline(always)]
    fn get_first_chunk_id(&self) -> ProviderResult<Option<ChunkId>> {
        self.provider()?.get_first_chunk_id()
    }
}

impl<DB: Database> SnapshotWriter for BotanixProviderFactory<DB> {
    fn create_new_snapshot_sync(
        &self,
        block_id: BlockNumber,
        snapshot_hash: B256,
        total_chunks: u64,
        format: u64,
    ) -> ProviderResult<SnapshotId> {
        let provider = self.provider_rw()?;

        let snapshot_id =
            provider.create_new_snapshot_sync(block_id, snapshot_hash, total_chunks, format)?;

        provider.commit()?;

        Ok(snapshot_id)
    }

    fn create_new_snapshot(
        &self,
        block_id: BlockNumber,
        block_hash: B256,
    ) -> ProviderResult<SnapshotId> {
        let provider = self.provider_rw()?;

        let snapshot_id = provider.create_new_snapshot(block_id, block_hash)?;

        provider.commit()?;

        Ok(snapshot_id)
    }

    fn create_new_chunk(
        &self,
        snapshot_id: SnapshotId,
        block_id: BlockNumber,
        chunk_data: Vec<u8>,
    ) -> ProviderResult<ChunkId> {
        let provider = self.provider_rw()?;

        let chunk_id = provider.create_new_chunk(snapshot_id, block_id, chunk_data)?;

        provider.commit()?;

        Ok(chunk_id)
    }

    fn append_to_chunk(
        &self,
        chunk_id: ChunkId,
        block_number: BlockNumber,
        data: Vec<u8>,
    ) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.append_to_chunk(chunk_id, block_number, data)?;

        provider.commit()?;

        Ok(())
    }

    fn update_snapshot(
        &self,
        snapshot_id: SnapshotId,
        block_id: BlockNumber,
        chunk_id: ChunkId,
    ) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.update_snapshot(snapshot_id, block_id, chunk_id)?;

        provider.commit()?;

        Ok(())
    }

    fn update_snapshot_sync(
        &self,
        snapshot_sync_id: SnapshotSyncId,
        updated_snapshot: SnapshotSync,
    ) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.update_snapshot_sync(snapshot_sync_id, updated_snapshot)?;

        provider.commit()?;

        Ok(())
    }

    fn remove_block_snapshot_id_mapping(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.remove_block_snapshot_id_mapping(range)?;

        provider.commit()?;

        Ok(())
    }

    fn insert_block_snapshot_id_mapping(
        &self,
        block_id: BlockNumber,
        snapshot_id: SnapshotId,
    ) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.insert_block_snapshot_id_mapping(block_id, snapshot_id)?;

        provider.commit()?;

        Ok(())
    }

    fn remove_snapshots(&self, range: RangeInclusive<SnapshotId>) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.remove_snapshots(range)?;

        provider.commit()?;

        Ok(())
    }

    fn remove_oldest_snapshot(&self) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.remove_oldest_snapshot()?;

        provider.commit()?;

        Ok(())
    }

    fn remove_chunks(&self, range: RangeInclusive<ChunkId>) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.remove_chunks(range)?;

        provider.commit()?;

        Ok(())
    }

    fn delete_chunks_in_blocks(&self, range: RangeInclusive<ChunkId>) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.delete_chunks_in_blocks(range)?;

        provider.commit()?;

        Ok(())
    }
}

impl<DB: Database> WalletStateSyncReader for BotanixProviderFactory<DB> {
    #[inline(always)]
    fn get_state_sync_records(&self) -> ProviderResult<Vec<WalletStateSyncRecord>> {
        self.provider()?.get_state_sync_records()
    }

    #[inline(always)]
    fn get_state_sync_record_peer_ids(&self) -> ProviderResult<Vec<PeerID>> {
        self.provider()?.get_state_sync_record_peer_ids()
    }

    #[inline(always)]
    fn get_state_sync_record_by_peer_id(
        &self,
        peer_id: PeerID,
    ) -> ProviderResult<Option<WalletStateSyncRecord>> {
        self.provider()?.get_state_sync_record_by_peer_id(peer_id)
    }

    #[inline(always)]
    fn get_state_sync_records_count(&self) -> ProviderResult<usize> {
        self.provider()?.get_state_sync_records_count()
    }

    #[inline(always)]
    fn get_minimum_superset(
        &self,
        min_required_criterion: u64,
    ) -> ProviderResult<(bool, HashSet<(u64, Bytes)>)> {
        self.provider()?.get_minimum_superset(min_required_criterion)
    }
}

impl<DB: Database> WalletStateSyncWriter for BotanixProviderFactory<DB> {
    fn create_new_state_sync_record(
        &self,
        uuid: UuidID,
        peer_id: PeerID,
        chunks_count: u64,
        data: Option<Vec<(u64, Bytes)>>,
    ) -> ProviderResult<PeerID> {
        let provider = self.provider_rw()?;

        let peer_id = provider.create_new_state_sync_record(uuid, peer_id, chunks_count, data)?;

        provider.commit()?;

        Ok(peer_id)
    }

    fn append_data_to_state_sync_record(
        &self,
        peer_id: PeerID,
        data: Vec<(u64, Bytes)>,
    ) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.append_data_to_state_sync_record(peer_id, data)?;

        provider.commit()?;

        Ok(())
    }

    fn remove_state_sync_record_per_peer_id(&self, peer_id: PeerID) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.remove_state_sync_record_per_peer_id(peer_id)?;

        provider.commit()?;

        Ok(())
    }

    fn remove_all_state_sync_records(&self) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.remove_all_state_sync_records()?;

        provider.commit()?;

        Ok(())
    }
}

impl<DB: Database> StagedHeaderReader for BotanixProviderFactory<DB> {
    #[inline(always)]
    fn get_staged_headers(&self) -> ProviderResult<Vec<(B256, HeaderWithPegs)>> {
        self.provider()?.get_staged_headers()
    }
}

impl<DB: Database> StagedHeaderWriter for BotanixProviderFactory<DB> {
    fn insert_staged_header(&self, id: B256, header: HeaderWithPegs) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.insert_staged_header(id, header)?;

        provider.commit()?;

        Ok(())
    }

    fn remove_staged_header(&self, id: B256) -> ProviderResult<bool> {
        let provider = self.provider_rw()?;

        let is_removed = provider.remove_staged_header(id)?;

        if is_removed {
            provider.commit()?;
        }

        Ok(is_removed)
    }
}

impl<DB: Database> RuntimeTransitionsReadWrite for BotanixProviderFactory<DB> {
    fn insert_runtime_upgrade_version(
        &self,
        height: BlockNumber,
        version: RuntimeVersion,
    ) -> ProviderResult<bool> {
        let provider = self.provider_rw()?;
        let did_change = provider.insert_runtime_upgrade_version(height, version)?;

        if did_change {
            provider.commit()?;
        }

        Ok(did_change)
    }

    fn get_runtime_versions(&self) -> ProviderResult<Vec<(BlockNumber, RuntimeVersion)>> {
        self.provider_rw()?.get_runtime_versions()
    }

    fn get_last_runtime_version(&self) -> ProviderResult<Option<RuntimeVersion>> {
        self.provider_rw()?.get_last_runtime_version()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::SnapshotReader;
    use reth_db::mdbx::DatabaseArguments;

    #[test]
    fn test_provider_factory_with_database_path() {
        let factory = BotanixProviderFactory::new_with_database_path(
            tempfile::TempDir::new().expect("can't create temp directory").into_path(),
            DatabaseArguments::new(Default::default()),
        )
        .unwrap();

        let provider = factory.provider().unwrap();
        provider.get_first_chunk_id().unwrap();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.get_first_chunk_id().unwrap();
    }
}
