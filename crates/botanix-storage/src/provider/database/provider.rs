use reth_db_api::{
    common::KeyValue,
    cursor::{DbCursorRO, DbCursorRW, RangeWalker},
    table::{Table, TableRow},
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_prune_types::PruneLimiter;
use reth_storage_errors::{db::DatabaseError, provider::ProviderResult};
use std::{
    fmt::Debug,
    ops::{Deref, DerefMut, RangeBounds},
};

/// A [`BotanixDatabaseProvider`] that holds a read-only database transaction.
pub type BotanixDatabaseProviderRO<DB> = BotanixDatabaseProvider<<DB as Database>::TX>;
/// A [`BotanixDatabaseProvider`] that holds a read-write database transaction.
///
/// Ideally this would be an alias type. However, there's some weird compiler error (<https://github.com/rust-lang/rust/issues/102211>), that forces us to wrap this in a struct instead.
/// Once that issue is solved, we can probably revert back to being an alias type.
#[derive(Debug)]
pub struct BotanixDatabaseProviderRW<DB: Database>(
    pub BotanixDatabaseProvider<<DB as Database>::TXMut>,
);

impl<DB: Database> Deref for BotanixDatabaseProviderRW<DB> {
    type Target = BotanixDatabaseProvider<<DB as Database>::TXMut>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<DB: Database> DerefMut for BotanixDatabaseProviderRW<DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<DB: Database> BotanixDatabaseProviderRW<DB> {
    /// Commit database transaction and static file if it exists.
    pub fn commit(self) -> ProviderResult<bool> {
        self.0.commit()
    }

    /// Consume `DbTx` or `DbTxMut`.
    pub fn into_tx(self) -> <DB as Database>::TXMut {
        self.0.into_tx()
    }
}

/// A provider struct that fetches data from the database.
/// 
/// This wrapper provides a unified interface around database transactions ([`DbTx`] and [`DbTxMut`])
/// for accessing Botanix storage data. It serves as the foundation for all database operations
/// in the storage system.
#[derive(Debug)]
pub struct BotanixDatabaseProvider<TX> {
    /// Database transaction.
    pub(super) tx: TX,
}

impl<TX: DbTxMut> BotanixDatabaseProvider<TX> {
    /// Creates a provider with an inner read-write transaction.
    pub const fn new_rw(tx: TX) -> Self {
        Self { tx }
    }
}

impl<TX: DbTx> BotanixDatabaseProvider<TX> {
    /// Creates a provider with an inner read-only transaction.
    pub const fn new(tx: TX) -> Self {
        Self { tx }
    }

    /// Consume `DbTx` or `DbTxMut`.
    pub fn into_tx(self) -> TX {
        self.tx
    }

    /// Pass `DbTx` or `DbTxMut` mutable reference.
    pub fn tx_mut(&mut self) -> &mut TX {
        &mut self.tx
    }

    /// Pass `DbTx` or `DbTxMut` immutable reference.
    pub const fn tx_ref(&self) -> &TX {
        &self.tx
    }

    /// Return full table as Vec
    pub fn table<T: Table>(&self) -> Result<Vec<KeyValue<T>>, DatabaseError>
    where
        T::Key: Default + Ord,
    {
        self.tx
            .cursor_read::<T>()?
            .walk(Some(T::Key::default()))?
            .collect::<Result<Vec<_>, DatabaseError>>()
    }

    /// Return a list of entries from the table, based on the given range.
    #[inline]
    pub fn get<T: Table>(
        &self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<Vec<KeyValue<T>>, DatabaseError> {
        self.tx.cursor_read::<T>()?.walk_range(range)?.collect::<Result<Vec<_>, _>>()
    }
}

impl<TX: DbTxMut + DbTx> BotanixDatabaseProvider<TX> {
    /// Commit database transaction.
    pub fn commit(self) -> ProviderResult<bool> {
        Ok(self.tx.commit()?)
    }

    /// Remove list of entries from the table. Returns the number of entries removed.
    #[inline]
    pub fn remove<T: Table>(
        &self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<usize, DatabaseError> {
        let mut entries = 0;
        let mut cursor_write = self.tx.cursor_write::<T>()?;
        let mut walker = cursor_write.walk_range(range)?;
        while walker.next().transpose()?.is_some() {
            walker.delete_current()?;
            entries += 1;
        }
        Ok(entries)
    }

    /// Return a list of entries from the table, and remove them, based on the given range.
    #[inline]
    pub fn take<T: Table>(
        &self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<Vec<KeyValue<T>>, DatabaseError> {
        let mut cursor_write = self.tx.cursor_write::<T>()?;
        let mut walker = cursor_write.walk_range(range)?;
        let mut items = Vec::new();
        while let Some(i) = walker.next().transpose()? {
            walker.delete_current()?;
            items.push(i)
        }
        Ok(items)
    }

    /// Unwind table by some number key.
    /// Returns number of rows unwound.
    ///
    /// Note: Key is not inclusive and specified key would stay in db.
    #[inline]
    pub fn unwind_table_by_num<T>(&self, num: u64) -> Result<usize, DatabaseError>
    where
        T: Table<Key = u64>,
    {
        self.unwind_table::<T, _>(num, |key| key)
    }

    /// Unwind the table to a provided number key.
    /// Returns number of rows unwound.
    ///
    /// Note: Key is not inclusive and specified key would stay in db.
    pub(crate) fn unwind_table<T, F>(
        &self,
        key: u64,
        mut selector: F,
    ) -> Result<usize, DatabaseError>
    where
        T: Table,
        F: FnMut(T::Key) -> u64,
    {
        let mut cursor = self.tx.cursor_write::<T>()?;
        let mut reverse_walker = cursor.walk_back(None)?;
        let mut deleted = 0;

        while let Some(Ok((entry_key, _))) = reverse_walker.next() {
            if selector(entry_key.clone()) <= key {
                break
            }
            reverse_walker.delete_current()?;
            deleted += 1;
        }

        Ok(deleted)
    }

    /// Unwind a table forward by a [`Walker`][reth_db_api::cursor::Walker] on another table
    pub fn unwind_table_by_walker<T1, T2>(
        &self,
        range: impl RangeBounds<T1::Key>,
    ) -> Result<(), DatabaseError>
    where
        T1: Table,
        T2: Table<Key = T1::Value>,
    {
        let mut cursor = self.tx.cursor_write::<T1>()?;
        let mut walker = cursor.walk_range(range)?;
        while let Some((_, value)) = walker.next().transpose()? {
            self.tx.delete::<T2>(value, None)?;
        }
        Ok(())
    }

    /// Prune the table for the specified pre-sorted key iterator.
    ///
    /// Returns number of rows pruned.
    pub fn prune_table_with_iterator<T: Table>(
        &self,
        keys: impl IntoIterator<Item = T::Key>,
        limiter: &mut PruneLimiter,
        mut delete_callback: impl FnMut(TableRow<T>),
    ) -> Result<(usize, bool), DatabaseError> {
        let mut cursor = self.tx.cursor_write::<T>()?;
        let mut keys = keys.into_iter();

        let mut deleted_entries = 0;

        for key in &mut keys {
            if limiter.is_limit_reached() {
                tracing::debug!(
                    target: "providers::db",
                    ?limiter,
                    deleted_entries_limit = %limiter.is_deleted_entries_limit_reached(),
                    time_limit = %limiter.is_time_limit_reached(),
                    table = %T::NAME,
                    "Pruning limit reached"
                );
                break
            }

            let row = cursor.seek_exact(key)?;
            if let Some(row) = row {
                cursor.delete_current()?;
                limiter.increment_deleted_entries_count();
                deleted_entries += 1;
                delete_callback(row);
            }
        }

        let done = keys.next().is_none();
        Ok((deleted_entries, done))
    }

    /// Prune the table for the specified key range.
    ///
    /// Returns number of rows pruned.
    pub fn prune_table_with_range<T: Table>(
        &self,
        keys: impl RangeBounds<T::Key> + Clone + Debug,
        limiter: &mut PruneLimiter,
        mut skip_filter: impl FnMut(&TableRow<T>) -> bool,
        mut delete_callback: impl FnMut(TableRow<T>),
    ) -> Result<(usize, bool), DatabaseError> {
        let mut cursor = self.tx.cursor_write::<T>()?;
        let mut walker = cursor.walk_range(keys)?;

        let mut deleted_entries = 0;

        let done = loop {
            // check for time out must be done in this scope since it's not done in
            // `prune_table_with_range_step`
            if limiter.is_limit_reached() {
                tracing::debug!(
                    target: "providers::db",
                    ?limiter,
                    deleted_entries_limit = %limiter.is_deleted_entries_limit_reached(),
                    time_limit = %limiter.is_time_limit_reached(),
                    table = %T::NAME,
                    "Pruning limit reached"
                );
                break false
            }

            let done = self.prune_table_with_range_step(
                &mut walker,
                limiter,
                &mut skip_filter,
                &mut delete_callback,
            )?;

            if done {
                break true
            } else {
                deleted_entries += 1;
            }
        };

        Ok((deleted_entries, done))
    }

    /// Steps once with the given walker and prunes the entry in the table.
    ///
    /// Returns `true` if the walker is finished, `false` if it may have more data to prune.
    ///
    /// CAUTION: Pruner limits are not checked. This allows for a clean exit of a prune run that's
    /// pruning different tables concurrently, by letting them step to the same height before
    /// timing out.
    pub fn prune_table_with_range_step<T: Table>(
        &self,
        walker: &mut RangeWalker<'_, T, <TX as DbTxMut>::CursorMut<T>>,
        limiter: &mut PruneLimiter,
        skip_filter: &mut impl FnMut(&TableRow<T>) -> bool,
        delete_callback: &mut impl FnMut(TableRow<T>),
    ) -> Result<bool, DatabaseError> {
        let Some(res) = walker.next() else { return Ok(true) };

        let row = res?;

        if !skip_filter(&row) {
            walker.delete_current()?;
            limiter.increment_deleted_entries_count();
            delete_callback(row);
        }

        Ok(false)
    }
}
