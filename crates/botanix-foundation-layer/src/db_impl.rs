use botanix_storage::{
    BotanixDatabaseProviderRW, BotanixProviderFactory, DatabaseProviderFactoryRW,
    FoundationLayerReader, FoundationLayerWriter,
};
// TODO: Consider adding a convenience `prelude::*` export module.
use botanix_tem::{
    foundation::{
        bitcoin::{BlockHash, OutPoint, Txid},
        hash_db,
        trie_db::{self, DBValue, HashDB},
        AtomicCommitLayer, AtomicDataLayer, AtomicError, CommitHasher, CommitLayerError,
        CommitmentStateRoot, DataSource, DataSourceError, FatDB, OnchainHeaderEntry,
        OnchainUtxoEntry, ProposalEntry, TrieLayer, UnassignedEntry,
    },
    validation::pegout::PegoutId,
};
use reth_db_api::Database;
use reth_storage_errors::provider::ProviderError;

/// A wrapper over the [`BotanixProviderFactory`] that implements both the
/// [`AtomicCommitLayer`] and [`AtomicDataLayer`] from the Botanix TEM crate.
///
/// This is passed-on to the [`botanix_tem::foundation::Foundation`] structure.
pub struct WBotanixProviderFactory<DB>
where
    DB: Database,
{
    /// The factory to acquire new database transactions from.
    factory: BotanixProviderFactory<DB>,
    /// The handle to the database transaction after calling
    /// `AtomicCommitLayer::start_trie_tx`.
    ///
    /// This is used to commit or rollback any changes made to the foundation
    /// trie/commitment layer.
    trie: Option<FatDB<WBotanixDatabaseProvider<DB>>>,
    /// The cached trie root. Only updated on database commit.
    cached_root: Option<CommitmentStateRoot>,
    /// The handle to the database transaction after calling
    /// `AtomicDataLayer::start_data_tx`.
    ///
    /// This is used to commit or rollback any changes made to the foundation
    /// data storage layer.
    data: Option<WBotanixDatabaseProvider<DB>>,
}

impl<DB> WBotanixProviderFactory<DB>
where
    DB: Database,
{
    /// Returns the latest trie root, using a cached value when available.
    ///
    /// The cached root is updated on every database commit. If no cached root
    /// exists, this method starts a **new database transaction** to fetch and
    /// cache the last committed root from the database.
    ///
    /// If no root exists in the database, this assumes an empty trie and returns
    /// the hashed null-node as defined by [`CommitHasher::HASHED_NULL_NODE`].
    ///
    /// Note: This method only retrieves the root value. Any trie operation
    /// errors or root computation errors will occur later during actual I/O
    /// operations on the trie, not in this method.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError`] if the database transaction or root retrieval
    /// fails.
    pub fn prepare_cached_root(&mut self) -> Result<CommitmentStateRoot, ProviderError> {
        if let Some(root) = self.cached_root {
            return Ok(root)
        }

        // Start a new database transaction.
        let provider = WBotanixDatabaseProvider { tx: self.factory.provider_rw()? };

        // Retrieve the root from the local database.
        let stored_root =
            provider.tx.get_foundation_commitment_root()?.unwrap_or(CommitHasher::HASHED_NULL_NODE);

        // If this is an empty database, then the null-node must be inserted
        // into the database if it does not exist yet. This is required by the
        // `trie-db`.
        if stored_root == CommitHasher::HASHED_NULL_NODE {
            provider.tx.insert_foundation_commitment(stored_root, vec![0u8])?;
        }

        // Retrieve the [`CommitmentStateRoot`] directly from the underlying trie-db.
        let mut trie = FatDB::from_existing(provider, stored_root);
        let root = trie.root();

        debug_assert_eq!(root.as_ref(), &stored_root);

        if stored_root == CommitHasher::HASHED_NULL_NODE {
            // Only commit database transaction if we have a reason to -
            // otherwise rollback.
            trie.into_db().tx.commit()?;
        }

        // Cache the latest root.
        self.cached_root = Some(root);

        Ok(root)
    }
}

impl<DB> From<BotanixProviderFactory<DB>> for WBotanixProviderFactory<DB>
where
    DB: Database,
{
    fn from(factory: BotanixProviderFactory<DB>) -> Self {
        WBotanixProviderFactory { factory, trie: None, cached_root: None, data: None }
    }
}

impl<DB> AtomicCommitLayer for WBotanixProviderFactory<DB>
where
    DB: Database,
{
    type BackendError = ProviderError;

    fn commit(&mut self) -> Result<CommitmentStateRoot, CommitLayerError<Self::BackendError>> {
        let mut trie = self.trie.take().ok_or(AtomicError::CommitmentLayerNotStarted)?;
        let root = trie.root();

        let provider: WBotanixDatabaseProvider<_> = trie.into_db();

        // Store the latest root directly into the transaction.
        provider
            .tx
            .insert_foundation_commitment_root(*root.as_ref())
            .map_err(AtomicError::Backend)?;

        // COMMIT the transaction changes to the database.
        provider.tx.commit().map_err(AtomicError::Backend)?;

        // Cache the latest root.
        self.cached_root = Some(root);

        debug_assert!(self.trie.is_none());
        Ok(root)
    }
    fn rollback(&mut self) -> Result<CommitmentStateRoot, CommitLayerError<Self::BackendError>> {
        let trie = self.trie.take().ok_or(AtomicError::CommitmentLayerNotStarted)?;

        // Just drop the database transaction; rollback is implied.
        let trash: WBotanixDatabaseProvider<_> = trie.into_db();
        std::mem::drop(trash);

        let root = self.root()?;

        debug_assert!(self.trie.is_none());
        Ok(root)
    }
    fn root(&mut self) -> Result<CommitmentStateRoot, CommitLayerError<Self::BackendError>> {
        self.prepare_cached_root().map_err(|err| AtomicError::Backend(err).into())
    }
    fn start_trie_tx<'db>(
        &'db mut self,
    ) -> Result<TrieLayer<'db>, CommitLayerError<Self::BackendError>> {
        if self.trie.is_some() {
            return Err(AtomicError::CommitmentLayerAlreadyStarted.into());
        }

        // NOTE: This starts a new database transaction on startup (only!),
        // which does result in a dead-lock IF called after the `let provider = ...`
        // call next.
        let root = self.root()?;

        // Start a new database transaction.
        let provider = WBotanixDatabaseProvider {
            tx: self.factory.provider_rw().map_err(AtomicError::Backend)?,
        };

        // Setup trie layer.
        self.trie = Some(FatDB::from_existing(provider, *root.as_ref()));
        let handle = self.trie.as_mut().expect("trie db must exist");

        Ok(handle.trie_layer())
    }
}

impl<DB> AtomicDataLayer for WBotanixProviderFactory<DB>
where
    DB: Database,
{
    type BackendError = ProviderError;
    type Transaction = WBotanixDatabaseProvider<DB>;

    // TODO: AtomicError variants should be more generic; rename to
    // `TransactionAlreadyStarted` or so.
    fn commit(
        &mut self,
    ) -> Result<(), botanix_tem::foundation::DataLayerError<Self::BackendError>> {
        let provider: WBotanixDatabaseProvider<_> =
            self.data.take().ok_or(AtomicError::CommitmentLayerNotStarted)?;

        // COMMIT the transaction changes to the database.
        provider.tx.commit().map_err(AtomicError::Backend)?;

        debug_assert!(self.data.is_none());
        Ok(())
    }
    fn rollback(
        &mut self,
    ) -> Result<(), botanix_tem::foundation::DataLayerError<Self::BackendError>> {
        // Just drop the database transaction; rollback is implied.
        let trash: WBotanixDatabaseProvider<_> =
            self.data.take().ok_or(AtomicError::CommitmentLayerNotStarted)?;

        std::mem::drop(trash);

        debug_assert!(self.data.is_none());
        Ok(())
    }
    fn start_data_tx<'tx>(
        &'tx mut self,
    ) -> Result<
        &'tx mut Self::Transaction,
        botanix_tem::foundation::DataLayerError<Self::BackendError>,
    > {
        if self.trie.is_some() {
            return Err(AtomicError::CommitmentLayerAlreadyStarted.into());
        }

        // Start a new database transaction.
        let provider = WBotanixDatabaseProvider {
            tx: self.factory.provider_rw().map_err(|err| AtomicError::Backend(err))?,
        };

        // Setup data layer.
        self.data = Some(provider);
        let provider = self.data.as_mut().expect("provider must be set");

        Ok(provider)
    }
}

/// A wrapper over the [`BotanixDatabaseProviderRW`] that implements both the
/// [`DataSource`] from the Botanix TEM and the [`trie_db::HashDB`] from the
/// Parity crate.
///
/// This is acquired internally in the [`botanix_tem::foundation::Foundation`]
/// structure via the [`WBotanixProviderFactory`].
pub struct WBotanixDatabaseProvider<DB>
where
    DB: Database,
{
    tx: BotanixDatabaseProviderRW<DB>,
}

impl<DB> hash_db::AsHashDB<CommitHasher, DBValue> for WBotanixDatabaseProvider<DB>
where
    DB: Database,
{
    fn as_hash_db(&self) -> &dyn HashDB<CommitHasher, DBValue> {
        self
    }
    fn as_hash_db_mut<'a>(&'a mut self) -> &'a mut (dyn HashDB<CommitHasher, DBValue> + 'a) {
        self
    }
}

impl<DB> trie_db::HashDB<CommitHasher, DBValue> for WBotanixDatabaseProvider<DB>
where
    DB: Database,
{
    fn get(&self, key: &[u8; 32], _prefix: (&[u8], Option<u8>)) -> Option<DBValue> {
        self.tx.get_foundation_commitment(*key).expect("failed to get key")
    }
    fn contains(&self, key: &[u8; 32], _prefix: (&[u8], Option<u8>)) -> bool {
        // TODO: Consider implementing explicit `contains` method.
        let val = self.tx.get_foundation_commitment(*key).expect("failed to get key");
        val.is_some()
    }
    fn insert(&mut self, prefix: (&[u8], Option<u8>), value: &[u8]) -> [u8; 32] {
        let (slice, byte) = prefix;

        let mut h = CommitHasher::new(b"botanix:trie-database-provider");
        h.append_message(b"prefix", slice);
        if let Some(b) = byte {
            h.append_u64(b"prefix-extra", b as u64);
        }
        h.append_message(b"value", value);
        //
        let key = h.finalize();

        self.tx.insert_foundation_commitment(key, value.to_vec()).expect("failed to insert key");

        key
    }
    fn emplace(&mut self, key: [u8; 32], _prefix: (&[u8], Option<u8>), value: DBValue) {
        self.tx.insert_foundation_commitment(key, value).expect("failed to insert key");
    }
    fn remove(&mut self, key: &[u8; 32], _prefix: (&[u8], Option<u8>)) {
        let did_remove = self.tx.remove_foundation_commitment(*key).expect("failed to delete key");
        debug_assert!(did_remove);
    }
}

impl<DB> DataSource for WBotanixDatabaseProvider<DB>
where
    DB: Database,
{
    type Error = ProviderError;

    fn insert_unassigned(
        &mut self,
        pegout: &PegoutId,
        entry: UnassignedEntry,
    ) -> Result<(), DataSourceError<Self::Error>> {
        self.tx.insert_unassigned_pegout(*pegout, entry).map_err(Into::into)
    }
    fn get_unassigned(
        &mut self,
        pegout: &PegoutId,
    ) -> Result<Option<UnassignedEntry>, DataSourceError<Self::Error>> {
        self.tx.get_unassigned_pegout(*pegout).map_err(Into::into)
    }
    fn remove_unassigned(&mut self, pegout: &PegoutId) -> Result<(), DataSourceError<Self::Error>> {
        self.tx.remove_unassigned_pegout(*pegout).map(|_| ()).map_err(Into::into)
    }
    fn insert_utxo(
        &mut self,
        utxo: &OutPoint,
        entry: OnchainUtxoEntry,
    ) -> Result<(), DataSourceError<Self::Error>> {
        self.tx.insert_onchain_utxo(*utxo, entry).map_err(Into::into)
    }
    fn get_utxo(
        &mut self,
        utxo: &OutPoint,
    ) -> Result<Option<OnchainUtxoEntry>, DataSourceError<Self::Error>> {
        self.tx.get_onchain_utxo(*utxo).map_err(Into::into)
    }
    fn finalize_utxo(&mut self, utxo: &OutPoint) -> Result<(), DataSourceError<Self::Error>> {
        self.tx.remove_onchain_utxo(*utxo).map(|_| ()).map_err(Into::into)
    }
    fn orphan_utxo(&mut self, utxo: &OutPoint) -> Result<(), DataSourceError<Self::Error>> {
        self.tx.remove_onchain_utxo(*utxo).map(|_| ()).map_err(Into::into)
    }
    fn insert_header(
        &mut self,
        block: &BlockHash,
        entry: OnchainHeaderEntry,
    ) -> Result<(), DataSourceError<Self::Error>> {
        self.tx.insert_onchain_header(*block, entry).map_err(Into::into)
    }
    fn get_header(
        &mut self,
        block: &BlockHash,
    ) -> Result<Option<OnchainHeaderEntry>, DataSourceError<Self::Error>> {
        self.tx.get_onchain_header(*block).map_err(Into::into)
    }
    fn remove_header(&mut self, block: &BlockHash) -> Result<(), DataSourceError<Self::Error>> {
        self.tx.remove_onchain_header(*block).map(|_| ()).map_err(Into::into)
    }
    fn insert_pegout_proposal(
        &mut self,
        txid: &Txid,
        entry: ProposalEntry,
    ) -> Result<(), DataSourceError<Self::Error>> {
        self.tx.insert_pegout_proposal(*txid, entry).map_err(Into::into)
    }
    fn get_proposal(
        &mut self,
        txid: &Txid,
    ) -> Result<Option<ProposalEntry>, DataSourceError<Self::Error>> {
        self.tx.get_pegout_proposal(*txid).map_err(Into::into)
    }
    fn finalize_proposal(&mut self, txid: &Txid) -> Result<(), DataSourceError<Self::Error>> {
        self.tx.remove_pegout_proposal(*txid).map(|_| ()).map_err(Into::into)
    }
    fn orphan_proposal(&mut self, txid: &Txid) -> Result<(), DataSourceError<Self::Error>> {
        self.tx.remove_pegout_proposal(*txid).map(|_| ()).map_err(Into::into)
    }
}
