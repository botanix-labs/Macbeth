use crate::{
    providers::{state::latest::LatestStateProvider, StaticFileProvider},
    to_range,
    traits::{BlockSource, ReceiptProvider},
    BlockHashReader, BlockNumReader, BlockReader, ChainSpecProvider, DatabaseProviderFactory,
    EvmEnvProvider, HeaderProvider, HeaderSyncGap, HeaderSyncGapProvider, ProviderError,
    PruneCheckpointReader, RequestsProvider, RuntimeTransitionsReadWrite, SnapshotReader,
    SnapshotWriter, StageCheckpointReader, StagedHeader, StateProviderBox,
    StaticFileProviderFactory, TransactionVariant, TransactionsProvider, WalletStateSyncReader,
    WalletStateSyncWriter, WithdrawalsProvider,
};
use reth_chainspec::{ChainInfo, ChainSpec, EthChainSpec};
use reth_db::{
    init_db,
    mdbx::DatabaseArguments,
    models::{
        ChunkId, HeaderWithPegs, PeerID, Snapshot, SnapshotChunk, SnapshotId, SnapshotSync,
        SnapshotSyncId, UuidID, WalletStateSyncRecord,
    },
    DatabaseEnv,
};
use reth_db_api::{database::Database, models::StoredBlockBodyIndices};
use reth_errors::{RethError, RethResult};
use reth_evm::ConfigureEvmEnv;
use reth_primitives::{
    Address, Block, BlockHash, BlockHashOrNumber, BlockNumber, BlockWithSenders, Bytes, Header,
    Receipt, SealedBlock, SealedBlockWithSenders, SealedHeader, StaticFileSegment, TransactionMeta,
    TransactionSigned, TransactionSignedNoHash, TxHash, TxNumber, Withdrawal, Withdrawals, B256,
    U256,
};
use reth_prune_types::{PruneCheckpoint, PruneModes, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_errors::provider::ProviderResult;
use revm::primitives::{BlockEnv, CfgEnvWithHandlerCfg};
use std::{
    collections::HashSet,
    ops::{RangeBounds, RangeInclusive},
    path::Path,
    sync::Arc,
};
use tokio::sync::watch;
use tracing::trace;

mod provider;
pub use provider::{DatabaseProvider, DatabaseProviderRO, DatabaseProviderRW};

mod metrics;

/// A common provider that fetches data from a database or static file.
///
/// This provider implements most provider or provider factory traits.
#[derive(Debug)]
pub struct ProviderFactory<DB, Spec = ChainSpec> {
    /// Database
    db: Arc<DB>,
    /// Chain spec
    chain_spec: Arc<Spec>,
    /// Static File Provider
    static_file_provider: StaticFileProvider,
    /// Optional pruning configuration
    prune_modes: PruneModes,
}

impl<DB> ProviderFactory<DB> {
    /// Create new database provider factory.
    pub fn new(
        db: DB,
        chain_spec: Arc<ChainSpec>,
        static_file_provider: StaticFileProvider,
    ) -> Self {
        Self { db: Arc::new(db), chain_spec, static_file_provider, prune_modes: PruneModes::none() }
    }

    /// Enables metrics on the static file provider.
    pub fn with_static_files_metrics(mut self) -> Self {
        self.static_file_provider = self.static_file_provider.with_metrics();
        self
    }

    /// Sets the pruning configuration for an existing [`ProviderFactory`].
    pub fn with_prune_modes(mut self, prune_modes: PruneModes) -> Self {
        self.prune_modes = prune_modes;
        self
    }

    /// Returns reference to the underlying database.
    pub fn db_ref(&self) -> &DB {
        self.db.as_ref()
    }

    #[cfg(any(test, feature = "test-utils"))]
    /// Consumes Self and returns DB
    pub fn into_db(self) -> Arc<DB> {
        self.db
    }
}

impl ProviderFactory<DatabaseEnv> {
    /// Create new database provider by passing a path. [`ProviderFactory`] will own the database
    /// instance.
    pub fn new_with_database_path<P: AsRef<Path>>(
        path: P,
        chain_spec: Arc<ChainSpec>,
        args: DatabaseArguments,
        static_file_provider: StaticFileProvider,
    ) -> RethResult<Self> {
        Ok(Self {
            db: Arc::new(init_db(path, args).map_err(RethError::msg)?),
            chain_spec,
            static_file_provider,
            prune_modes: PruneModes::none(),
        })
    }
}

impl<DB: Database> ProviderFactory<DB> {
    /// Returns a provider with a created `DbTx` inside, which allows fetching data from the
    /// database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`]. This may fail if the inner read database transaction fails to open.
    ///
    /// This sets the [`PruneModes`] to [`None`], because they should only be relevant for writing
    /// data.
    #[track_caller]
    pub fn provider(&self) -> ProviderResult<DatabaseProviderRO<DB>> {
        Ok(DatabaseProvider::new(
            self.db.tx()?,
            self.chain_spec.clone(),
            self.static_file_provider.clone(),
            self.prune_modes.clone(),
        ))
    }

    /// Returns a provider with a created `DbTxMut` inside, which allows fetching and updating
    /// data from the database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`].  This may fail if the inner read/write database transaction fails to
    /// open.
    #[track_caller]
    pub fn provider_rw(&self) -> ProviderResult<DatabaseProviderRW<DB>> {
        Ok(DatabaseProviderRW(DatabaseProvider::new_rw(
            self.db.tx_mut()?,
            self.chain_spec.clone(),
            self.static_file_provider.clone(),
            self.prune_modes.clone(),
        )))
    }

    /// State provider for latest block
    #[track_caller]
    pub fn latest(&self) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::db", "Returning latest state provider");
        Ok(Box::new(LatestStateProvider::new(self.db.tx()?, self.static_file_provider())))
    }

    /// Storage provider for state at that given block
    pub fn history_by_block_number(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<StateProviderBox> {
        let state_provider = self.provider()?.state_provider_by_block_number(block_number)?;
        trace!(target: "providers::db", ?block_number, "Returning historical state provider for block number");
        Ok(state_provider)
    }

    /// Storage provider for state at that given block hash
    pub fn history_by_block_hash(&self, block_hash: BlockHash) -> ProviderResult<StateProviderBox> {
        let provider = self.provider()?;

        let block_number = provider
            .block_number(block_hash)?
            .ok_or(ProviderError::BlockHashNotFound(block_hash))?;

        let state_provider = self.provider()?.state_provider_by_block_number(block_number)?;
        trace!(target: "providers::db", ?block_number, %block_hash, "Returning historical state provider for block hash");
        Ok(state_provider)
    }
}

impl<DB: Database> DatabaseProviderFactory<DB> for ProviderFactory<DB> {
    fn database_provider_ro(&self) -> ProviderResult<DatabaseProviderRO<DB>> {
        self.provider()
    }
}

impl<DB> StaticFileProviderFactory for ProviderFactory<DB> {
    /// Returns static file provider
    fn static_file_provider(&self) -> StaticFileProvider {
        self.static_file_provider.clone()
    }
}

impl<DB: Database> HeaderSyncGapProvider for ProviderFactory<DB> {
    fn sync_gap(
        &self,
        tip: watch::Receiver<B256>,
        highest_uninterrupted_block: BlockNumber,
    ) -> ProviderResult<HeaderSyncGap> {
        self.provider()?.sync_gap(tip, highest_uninterrupted_block)
    }
}

impl<DB: Database> HeaderProvider for ProviderFactory<DB> {
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        self.provider()?.header(block_hash)
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Header>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            num,
            |static_file| static_file.header_by_number(num),
            || self.provider()?.header_by_number(num),
        )
    }

    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>> {
        self.provider()?.header_td(hash)
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        if let Some(td) = self.chain_spec.final_paris_total_difficulty(number) {
            // if this block is higher than the final paris(merge) block, return the final paris
            // difficulty
            return Ok(Some(td));
        }

        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.header_td_by_number(number),
            || self.provider()?.header_td_by_number(number),
        )
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            to_range(range),
            |static_file, range, _| static_file.headers_range(range),
            |range, _| self.provider()?.headers_range(range),
            |_| true,
        )
    }

    fn sealed_header(&self, number: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.sealed_header(number),
            || self.provider()?.sealed_header(number),
        )
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.sealed_headers_while(range, |_| true)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            to_range(range),
            |static_file, range, predicate| static_file.sealed_headers_while(range, predicate),
            |range, predicate| self.provider()?.sealed_headers_while(range, predicate),
            predicate,
        )
    }
}

impl<DB: Database> SnapshotReader for ProviderFactory<DB> {
    fn get_snapshots(&self) -> ProviderResult<Vec<Snapshot>> {
        self.provider()?.get_snapshots()
    }

    fn get_snapshot_by_id(&self, snapshot_id: SnapshotId) -> ProviderResult<Option<Snapshot>> {
        self.provider()?.get_snapshot_by_id(snapshot_id)
    }

    fn get_last_snapshot_sync_id(&self) -> ProviderResult<Option<SnapshotSyncId>> {
        self.provider()?.get_last_snapshot_sync_id()
    }

    fn get_snapshot_sync_by_height(&self, height: u64) -> ProviderResult<Option<SnapshotSync>> {
        self.provider()?.get_snapshot_sync_by_height(height)
    }

    fn get_snapshot_sync_by_id(&self, id: u64) -> ProviderResult<Option<SnapshotSync>> {
        self.provider()?.get_snapshot_sync_by_id(id)
    }

    fn get_chunk_by_id(
        &self,
        chunk_id: reth_db::models::ChunkId,
    ) -> ProviderResult<Option<SnapshotChunk>> {
        self.provider()?.get_chunk_by_id(chunk_id)
    }

    fn get_chunk_size(&self, chunk_id: reth_db::models::ChunkId) -> ProviderResult<usize> {
        self.provider()?.get_chunk_size(chunk_id)
    }

    fn get_snapshot_id_by_block_id(
        &self,
        block_id: BlockNumber,
    ) -> ProviderResult<Option<SnapshotId>> {
        self.provider()?.get_snapshot_id_by_block_id(block_id)
    }

    fn get_chunk_block_number(&self, chunk_id: ChunkId) -> ProviderResult<Option<BlockNumber>> {
        self.provider()?.get_chunk_block_number(chunk_id)
    }

    fn get_last_snapshot_height(&self) -> ProviderResult<Option<(SnapshotId, BlockNumber)>> {
        self.provider()?.get_last_snapshot_height()
    }

    fn get_first_snapshot_height(&self) -> ProviderResult<Option<(SnapshotId, BlockNumber)>> {
        self.provider()?.get_first_snapshot_height()
    }

    fn get_snapshot_size(&self, snapshot_id: SnapshotId) -> ProviderResult<usize> {
        self.provider()?.get_snapshot_size(snapshot_id)
    }

    fn get_snapshots_count(&self) -> ProviderResult<usize> {
        self.provider()?.get_snapshots_count()
    }

    fn get_last_chunk_id(&self) -> ProviderResult<Option<ChunkId>> {
        self.provider()?.get_last_chunk_id()
    }

    fn get_first_chunk_id(&self) -> ProviderResult<Option<ChunkId>> {
        self.provider()?.get_first_chunk_id()
    }
}

impl<DB: Database> SnapshotWriter for ProviderFactory<DB> {
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

impl<DB: Database> WalletStateSyncReader for ProviderFactory<DB> {
    fn get_state_sync_records(&self) -> ProviderResult<Vec<WalletStateSyncRecord>> {
        self.provider()?.get_state_sync_records()
    }

    fn get_state_sync_record_peer_ids(&self) -> ProviderResult<Vec<PeerID>> {
        self.provider()?.get_state_sync_record_peer_ids()
    }

    fn get_state_sync_record_by_peer_id(
        &self,
        peer_id: PeerID,
    ) -> ProviderResult<Option<WalletStateSyncRecord>> {
        self.provider()?.get_state_sync_record_by_peer_id(peer_id)
    }

    fn get_state_sync_records_count(&self) -> ProviderResult<usize> {
        self.provider()?.get_state_sync_records_count()
    }

    fn get_minimum_superset(
        &self,
        min_required_criterion: u64,
    ) -> ProviderResult<(bool, HashSet<(u64, Bytes)>)> {
        self.provider()?.get_minimum_superset(min_required_criterion)
    }
}

impl<DB: Database> WalletStateSyncWriter for ProviderFactory<DB> {
    /// Create new state sync record
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

    /// Append data to state sync record
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

    /// Remove state sync record by `peer_id`
    fn remove_state_sync_record_per_peer_id(&self, peer_id: PeerID) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.remove_state_sync_record_per_peer_id(peer_id)?;

        provider.commit()?;

        Ok(())
    }

    /// Removes all state sync records
    fn remove_all_state_sync_records(&self) -> ProviderResult<()> {
        let provider = self.provider_rw()?;

        provider.remove_all_state_sync_records()?;

        provider.commit()?;

        Ok(())
    }
}

impl<DB: Database> BlockHashReader for ProviderFactory<DB> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.block_hash(number),
            || self.provider()?.block_hash(number),
        )
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            start..end,
            |static_file, range, _| static_file.canonical_hashes_range(range.start, range.end),
            |range, _| self.provider()?.canonical_hashes_range(range.start, range.end),
            |_| true,
        )
    }
}

impl<DB: Database> BlockNumReader for ProviderFactory<DB> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        self.provider()?.chain_info()
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        self.provider()?.best_block_number()
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        self.provider()?.last_block_number()
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        self.provider()?.block_number(hash)
    }
}

impl<DB: Database> BlockReader for ProviderFactory<DB> {
    fn find_block_by_hash(&self, hash: B256, source: BlockSource) -> ProviderResult<Option<Block>> {
        self.provider()?.find_block_by_hash(hash, source)
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Block>> {
        self.provider()?.block(id)
    }

    fn pending_block(&self) -> ProviderResult<Option<SealedBlock>> {
        self.provider()?.pending_block()
    }

    fn pending_block_with_senders(&self) -> ProviderResult<Option<SealedBlockWithSenders>> {
        self.provider()?.pending_block_with_senders()
    }

    fn pending_block_and_receipts(&self) -> ProviderResult<Option<(SealedBlock, Vec<Receipt>)>> {
        self.provider()?.pending_block_and_receipts()
    }

    fn ommers(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Header>>> {
        self.provider()?.ommers(id)
    }

    fn block_body_indices(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        self.provider()?.block_body_indices(number)
    }

    fn block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders>> {
        self.provider()?.block_with_senders(id, transaction_kind)
    }

    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<SealedBlockWithSenders>> {
        self.provider()?.sealed_block_with_senders(id, transaction_kind)
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Block>> {
        self.provider()?.block_range(range)
    }

    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockWithSenders>> {
        self.provider()?.block_with_senders_range(range)
    }

    fn sealed_block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<SealedBlockWithSenders>> {
        self.provider()?.sealed_block_with_senders_range(range)
    }
}

impl<DB: Database> TransactionsProvider for ProviderFactory<DB> {
    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.provider()?.transaction_id(tx_hash)
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<TransactionSigned>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Transactions,
            id,
            |static_file| static_file.transaction_by_id(id),
            || self.provider()?.transaction_by_id(id),
        )
    }

    fn transaction_by_id_no_hash(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<TransactionSignedNoHash>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Transactions,
            id,
            |static_file| static_file.transaction_by_id_no_hash(id),
            || self.provider()?.transaction_by_id_no_hash(id),
        )
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<TransactionSigned>> {
        self.provider()?.transaction_by_hash(hash)
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(TransactionSigned, TransactionMeta)>> {
        self.provider()?.transaction_by_hash_with_meta(tx_hash)
    }

    fn transaction_block(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        self.provider()?.transaction_block(id)
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<TransactionSigned>>> {
        self.provider()?.transactions_by_block(id)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<TransactionSigned>>> {
        self.provider()?.transactions_by_block_range(range)
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<TransactionSignedNoHash>> {
        self.provider()?.transactions_by_tx_range(range)
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        self.provider()?.senders_by_tx_range(range)
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        self.provider()?.transaction_sender(id)
    }
}

impl<DB: Database> ReceiptProvider for ProviderFactory<DB> {
    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Receipt>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Receipts,
            id,
            |static_file| static_file.receipt(id),
            || self.provider()?.receipt(id),
        )
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Receipt>> {
        self.provider()?.receipt_by_hash(hash)
    }

    fn receipts_by_block(&self, block: BlockHashOrNumber) -> ProviderResult<Option<Vec<Receipt>>> {
        self.provider()?.receipts_by_block(block)
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Receipt>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Receipts,
            to_range(range),
            |static_file, range, _| static_file.receipts_by_tx_range(range),
            |range, _| self.provider()?.receipts_by_tx_range(range),
            |_| true,
        )
    }
}

impl<DB: Database> WithdrawalsProvider for ProviderFactory<DB> {
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<Withdrawals>> {
        self.provider()?.withdrawals_by_block(id, timestamp)
    }

    fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>> {
        self.provider()?.latest_withdrawal()
    }
}

impl<DB> RequestsProvider for ProviderFactory<DB>
where
    DB: Database,
{
    fn requests_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<reth_primitives::Requests>> {
        self.provider()?.requests_by_block(id, timestamp)
    }
}

impl<DB: Database> StageCheckpointReader for ProviderFactory<DB> {
    fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        self.provider()?.get_stage_checkpoint(id)
    }

    fn get_stage_checkpoint_progress(&self, id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        self.provider()?.get_stage_checkpoint_progress(id)
    }
    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>> {
        self.provider()?.get_all_checkpoints()
    }
}

impl<DB: Database> EvmEnvProvider for ProviderFactory<DB> {
    fn fill_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv,
    {
        self.provider()?.fill_env_at(cfg, block_env, at, evm_config)
    }

    fn fill_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv,
    {
        self.provider()?.fill_env_with_header(cfg, block_env, header, evm_config)
    }

    fn fill_cfg_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv,
    {
        self.provider()?.fill_cfg_env_at(cfg, at, evm_config)
    }

    fn fill_cfg_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv,
    {
        self.provider()?.fill_cfg_env_with_header(cfg, header, evm_config)
    }
}

impl<DB, ChainSpec> ChainSpecProvider for ProviderFactory<DB, ChainSpec>
where
    DB: Send + Sync,
    ChainSpec: EthChainSpec,
{
    type ChainSpec = ChainSpec;
    fn chain_spec(&self) -> Arc<ChainSpec> {
        self.chain_spec.clone()
    }
}

impl<DB: Database> PruneCheckpointReader for ProviderFactory<DB> {
    fn get_prune_checkpoint(
        &self,
        segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>> {
        self.provider()?.get_prune_checkpoint(segment)
    }

    fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
        self.provider()?.get_prune_checkpoints()
    }
}

impl<DB, Spec> Clone for ProviderFactory<DB, Spec> {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
            chain_spec: self.chain_spec.clone(),
            static_file_provider: self.static_file_provider.clone(),
            prune_modes: self.prune_modes.clone(),
        }
    }
}

impl<DB: Database> StagedHeader for ProviderFactory<DB> {
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

    fn get_staged_headers(&self) -> ProviderResult<Vec<(B256, HeaderWithPegs)>> {
        self.provider_rw()?.get_staged_headers()
    }
}

impl<DB: Database> RuntimeTransitionsReadWrite for ProviderFactory<DB> {
    fn insert_runtime_upgrade_version(
        &self,
        height: BlockNumber,
        version: reth_db::models::RuntimeVersion,
    ) -> ProviderResult<bool> {
        let provider = self.provider_rw()?;
        let did_change = provider.insert_runtime_upgrade_version(height, version)?;

        if did_change {
            provider.commit()?;
        }

        Ok(did_change)
    }

    fn get_runtime_versions(
        &self,
    ) -> ProviderResult<Vec<(BlockNumber, reth_db::models::RuntimeVersion)>> {
        self.provider_rw()?.get_runtime_versions()
    }

    fn get_last_runtime_version(&self) -> ProviderResult<Option<reth_db::models::RuntimeVersion>> {
        self.provider_rw()?.get_last_runtime_version()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        providers::{StaticFileProvider, StaticFileWriter},
        test_utils::{blocks::TEST_BLOCK, create_test_provider_factory},
        BlockHashReader, BlockNumReader, BlockWriter, HeaderSyncGapProvider, TransactionsProvider,
    };
    use assert_matches::assert_matches;
    use rand::Rng;
    use reth_chainspec::ChainSpecBuilder;
    use reth_db::{
        mdbx::DatabaseArguments,
        tables,
        test_utils::{create_test_static_files_dir, ERROR_TEMPDIR},
    };
    use reth_primitives::{StaticFileSegment, TxNumber, B256, U256};
    use reth_prune_types::{PruneMode, PruneModes};
    use reth_storage_errors::provider::ProviderError;
    use reth_testing_utils::{
        generators,
        generators::{random_block, random_header},
    };
    use std::{ops::RangeInclusive, sync::Arc};
    use tokio::sync::watch;

    #[test]
    fn common_history_provider() {
        let factory = create_test_provider_factory();
        let _ = factory.latest();
    }

    #[test]
    fn default_chain_info() {
        let factory = create_test_provider_factory();
        let provider = factory.provider().unwrap();

        let chain_info = provider.chain_info().expect("should be ok");
        assert_eq!(chain_info.best_number, 0);
        assert_eq!(chain_info.best_hash, B256::ZERO);
    }

    #[test]
    fn provider_flow() {
        let factory = create_test_provider_factory();
        let provider = factory.provider().unwrap();
        provider.block_hash(0).unwrap();
        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.block_hash(0).unwrap();
        provider.block_hash(0).unwrap();
    }

    #[test]
    fn provider_factory_with_database_path() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let (_static_dir, static_dir_path) = create_test_static_files_dir();
        let factory = ProviderFactory::new_with_database_path(
            tempfile::TempDir::new().expect(ERROR_TEMPDIR).keep(),
            Arc::new(chain_spec),
            DatabaseArguments::new(Default::default()),
            StaticFileProvider::read_write(static_dir_path).unwrap(),
        )
        .unwrap();

        let provider = factory.provider().unwrap();
        provider.block_hash(0).unwrap();
        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.block_hash(0).unwrap();
        provider.block_hash(0).unwrap();
    }

    #[test]
    fn insert_block_with_prune_modes() {
        let factory = create_test_provider_factory();

        let block = TEST_BLOCK.clone();
        {
            let provider = factory.provider_rw().unwrap();
            assert_matches!(
                provider.insert_block(block.clone().try_seal_with_senders().unwrap()),
                Ok(_)
            );
            assert_matches!(
                provider.transaction_sender(0), Ok(Some(sender))
                if sender == block.body[0].recover_signer().unwrap()
            );
            assert_matches!(provider.transaction_id(block.body[0].hash), Ok(Some(0)));
        }

        {
            let prune_modes = PruneModes {
                sender_recovery: Some(PruneMode::Full),
                transaction_lookup: Some(PruneMode::Full),
                ..PruneModes::none()
            };
            let provider = factory.with_prune_modes(prune_modes).provider_rw().unwrap();
            assert_matches!(
                provider.insert_block(block.clone().try_seal_with_senders().unwrap(),),
                Ok(_)
            );
            assert_matches!(provider.transaction_sender(0), Ok(None));
            assert_matches!(provider.transaction_id(block.body[0].hash), Ok(None));
        }
    }

    #[test]
    fn take_block_transaction_range_recover_senders() {
        let factory = create_test_provider_factory();

        let mut rng = generators::rng();
        let block = random_block(&mut rng, 0, None, Some(3), None, None);

        let tx_ranges: Vec<RangeInclusive<TxNumber>> = vec![0..=0, 1..=1, 2..=2, 0..=1, 1..=2];
        for range in tx_ranges {
            let provider = factory.provider_rw().unwrap();

            assert_matches!(
                provider.insert_block(block.clone().try_seal_with_senders().unwrap()),
                Ok(_)
            );

            let senders = provider.take::<tables::TransactionSenders>(range.clone());
            assert_eq!(
                senders,
                Ok(range
                    .clone()
                    .map(|tx_number| (
                        tx_number,
                        block.body[tx_number as usize].recover_signer().unwrap()
                    ))
                    .collect())
            );

            let db_senders = provider.senders_by_tx_range(range);
            assert_eq!(db_senders, Ok(vec![]));

            let result = provider.take_block_transaction_range(0..=0);
            assert_eq!(
                result,
                Ok(vec![(
                    0,
                    block.body.iter().cloned().map(|tx| tx.into_ecrecovered().unwrap()).collect()
                )])
            )
        }
    }

    #[test]
    fn header_sync_gap_lookup() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let mut rng = generators::rng();
        let consensus_tip = rng.gen();
        let (_tip_tx, tip_rx) = watch::channel(consensus_tip);

        // Genesis
        let checkpoint = 0;
        let head = random_header(&mut rng, 0, None);

        // Empty database
        assert_matches!(
            provider.sync_gap(tip_rx.clone(), checkpoint),
            Err(ProviderError::HeaderNotFound(block_number))
                if block_number.as_number().unwrap() == checkpoint
        );

        // Checkpoint and no gap
        let mut static_file_writer =
            provider.static_file_provider().latest_writer(StaticFileSegment::Headers).unwrap();
        static_file_writer.append_header(head.header(), U256::ZERO, &head.hash()).unwrap();
        static_file_writer.commit().unwrap();
        drop(static_file_writer);

        let gap = provider.sync_gap(tip_rx, checkpoint).unwrap();
        assert_eq!(gap.local_head, head);
        assert_eq!(gap.target.tip(), consensus_tip.into());
    }
}
