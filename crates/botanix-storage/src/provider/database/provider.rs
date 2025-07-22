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
///
/// This type alias provides a convenient way to create database providers that
/// can only perform read operations. It wraps a read-only transaction type
/// from the underlying database.
///
/// # Usage
///
/// Use this type when you only need to read data from the database and want
/// to ensure no accidental writes can occur.
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
    ///
    /// Finalizes all pending write operations in the database transaction.
    /// This method consumes the provider to ensure the transaction is
    /// properly committed and cannot be used after commitment.
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - If the transaction was successfully committed
    /// * `Ok(false)` - If there was nothing to commit
    /// * `Err(ProviderError)` - If the commit operation failed
    ///
    /// # Important
    ///
    /// Changes are not persisted until this method is called successfully.
    /// Dropping the provider without calling commit will rollback all changes.
    pub fn commit(self) -> ProviderResult<bool> {
        self.0.commit()
    }

    /// Consume the provider and return the underlying database transaction.
    ///
    /// This method consumes the provider and returns the wrapped database
    /// transaction, allowing direct access to the transaction if needed.
    ///
    /// # Returns
    ///
    /// The underlying mutable database transaction.
    pub fn into_tx(self) -> <DB as Database>::TXMut {
        self.0.into_tx()
    }
}

/// A provider struct that fetches data from the database.
///
/// This wrapper provides a unified interface around database transactions ([`DbTx`] and
/// [`DbTxMut`]) for accessing Botanix storage data. It serves as the foundation for all database
/// operations in the storage system.
#[derive(Debug)]
pub struct BotanixDatabaseProvider<TX> {
    /// Database transaction.
    pub(super) tx: TX,
}

impl<TX: DbTxMut> BotanixDatabaseProvider<TX> {
    /// Creates a provider with an inner read-write transaction.
    ///
    /// Constructs a new database provider that can perform both read and write
    /// operations using the provided mutable transaction.
    ///
    /// # Parameters
    ///
    /// * `tx` - A mutable database transaction that supports both reads and writes
    ///
    /// # Returns
    ///
    /// A new `BotanixDatabaseProvider` instance capable of read-write operations.
    pub const fn new_rw(tx: TX) -> Self {
        Self { tx }
    }
}

impl<TX: DbTx> BotanixDatabaseProvider<TX> {
    /// Creates a provider with an inner read-only transaction.
    ///
    /// Constructs a new database provider that can only perform read operations
    /// using the provided read-only transaction.
    ///
    /// # Parameters
    ///
    /// * `tx` - A read-only database transaction
    ///
    /// # Returns
    ///
    /// A new `BotanixDatabaseProvider` instance for read-only operations.
    pub const fn new(tx: TX) -> Self {
        Self { tx }
    }

    /// Consume the provider and return the underlying database transaction.
    ///
    /// This method consumes the provider and returns the wrapped database
    /// transaction, allowing direct access to the transaction if needed.
    ///
    /// # Returns
    ///
    /// The underlying database transaction (either read-only or read-write).
    pub fn into_tx(self) -> TX {
        self.tx
    }

    /// Get a mutable reference to the underlying database transaction.
    ///
    /// Provides mutable access to the wrapped database transaction for operations
    /// that require direct transaction manipulation.
    ///
    /// # Returns
    ///
    /// A mutable reference to the underlying database transaction.
    pub fn tx_mut(&mut self) -> &mut TX {
        &mut self.tx
    }

    /// Get an immutable reference to the underlying database transaction.
    ///
    /// Provides read-only access to the wrapped database transaction for
    /// inspection or read operations that require direct transaction access.
    ///
    /// # Returns
    ///
    /// An immutable reference to the underlying database transaction.
    pub const fn tx_ref(&self) -> &TX {
        &self.tx
    }

    /// Return full table as Vec
    ///
    /// Retrieves all entries from a database table and returns them as a vector
    /// of key-value pairs. This method loads the entire table into memory.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The table type to read from
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<KeyValue<T>>)` - All entries in the table
    /// * `Err(DatabaseError)` - If there was a database access error
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
    ///
    /// Retrieves entries from a database table that fall within the specified
    /// key range. This allows for efficient querying of table subsets.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The table type to read from
    ///
    /// # Parameters
    ///
    /// * `range` - The range of keys to retrieve (can be inclusive, exclusive, or unbounded)
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<KeyValue<T>>)` - All entries within the specified range
    /// * `Err(DatabaseError)` - If there was a database access error
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // Get entries with keys from 100 to 200 (inclusive)
    /// let entries = provider.get::<MyTable>(100..=200)?;
    ///
    /// // Get all entries with keys >= 50
    /// let entries = provider.get::<MyTable>(50..)?;
    /// ```
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
    ///
    /// Finalizes all pending write operations in the database transaction.
    /// This method consumes the provider to ensure the transaction is
    /// properly committed and cannot be used after commitment.
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - If the transaction was successfully committed
    /// * `Ok(false)` - If there was nothing to commit
    /// * `Err(ProviderError)` - If the commit operation failed
    ///
    /// # Important
    ///
    /// Changes are not persisted until this method is called successfully.
    /// Dropping the provider without calling commit will rollback all changes.
    pub fn commit(self) -> ProviderResult<bool> {
        Ok(self.tx.commit()?)
    }

    /// Remove list of entries from the table. Returns the number of entries removed.
    ///
    /// Deletes all entries from a database table that fall within the specified
    /// key range. This is an efficient way to remove multiple entries at once.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The table type to remove entries from
    ///
    /// # Parameters
    ///
    /// * `range` - The range of keys to remove (can be inclusive, exclusive, or unbounded)
    ///
    /// # Returns
    ///
    /// * `Ok(usize)` - The number of entries that were successfully removed
    /// * `Err(DatabaseError)` - If there was a database access error
    ///
    /// # Warning
    ///
    /// This operation permanently deletes data. Ensure you have proper backups
    /// or are certain about the deletion before calling this method.
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
    ///
    /// Retrieves entries from a database table within the specified key range
    /// and simultaneously removes them from the table. This is an atomic operation
    /// that combines reading and deletion.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The table type to read from and remove entries from
    ///
    /// # Parameters
    ///
    /// * `range` - The range of keys to retrieve and remove
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<KeyValue<T>>)` - The entries that were retrieved and removed
    /// * `Err(DatabaseError)` - If there was a database access error
    ///
    /// # Usage
    ///
    /// This method is useful when you need to process and remove entries
    /// atomically, ensuring no entries are left behind after processing.
    ///
    /// # Warning
    ///
    /// The entries are permanently removed from the table. Ensure this is
    /// the intended behavior before calling this method.
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
    ///
    /// Removes all entries from a table that have keys greater than the specified
    /// number. This is commonly used for blockchain reorganizations where you need
    /// to remove data after a certain block number.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The table type with u64 keys to unwind
    ///
    /// # Parameters
    ///
    /// * `num` - The key threshold (entries with keys > num are removed)
    ///
    /// # Returns
    ///
    /// * `Ok(usize)` - The number of entries that were removed
    /// * `Err(DatabaseError)` - If there was a database access error
    ///
    /// # Important
    ///
    /// The specified key is NOT inclusive - entries with the exact key value
    /// will remain in the database. Only entries with keys > num are removed.
    #[inline]
    pub fn unwind_table_by_num<T>(&self, num: u64) -> Result<usize, DatabaseError>
    where
        T: Table<Key = u64>,
    {
        self.unwind_table::<T, _>(num, |key| key)
    }

    /// Unwind the table to a provided number key.
    ///
    /// Removes all entries from a table where the selector function returns
    /// a value greater than the specified key. This provides flexibility for
    /// unwinding tables with complex key types.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The table type to unwind
    /// * `F` - Function type that extracts a u64 value from table keys
    ///
    /// # Parameters
    ///
    /// * `key` - The threshold value (entries where selector(entry_key) > key are removed)
    /// * `selector` - Function that extracts a u64 value from each table key
    ///
    /// # Returns
    ///
    /// * `Ok(usize)` - The number of entries that were removed
    /// * `Err(DatabaseError)` - If there was a database access error
    ///
    /// # Behavior
    ///
    /// The method walks backwards through the table and removes entries where
    /// the selector function returns a value greater than the specified key.
    /// This allows for efficient unwinding from the end of the table.
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
    ///
    /// Removes entries from table T2 using values from table T1 within the specified range.
    /// This method walks through table T1 and uses each value as a key to delete
    /// corresponding entries in table T2. This is useful for maintaining referential
    /// integrity when unwinding related tables.
    ///
    /// # Type Parameters
    ///
    /// * `T1` - The source table to walk through
    /// * `T2` - The target table to delete from (T2::Key must match T1::Value)
    ///
    /// # Parameters
    ///
    /// * `range` - The range of keys to process in table T1
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If all deletions completed successfully
    /// * `Err(DatabaseError)` - If there was a database access error
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
    /// Efficiently removes entries from a table using a pre-sorted iterator of keys.
    /// This method respects pruning limits and provides callback functionality for
    /// tracking deleted entries. It's designed for batch deletion operations where
    /// the keys to delete are known in advance.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The table type to prune entries from
    ///
    /// # Parameters
    ///
    /// * `keys` - Iterator of keys to delete (must be pre-sorted for efficiency)
    /// * `limiter` - Pruning limiter that controls how many entries can be deleted
    /// * `delete_callback` - Function called for each deleted row (useful for logging/tracking)
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// * `usize` - The number of entries that were successfully deleted
    /// * `bool` - Whether all keys were processed (true) or pruning was stopped due to limits
    ///   (false)
    ///
    /// # Behavior
    ///
    /// - Processes keys in the order provided by the iterator
    /// - Stops immediately if pruning limits are reached
    /// - Calls the delete callback for each successfully deleted row
    /// - Skips keys that don't exist in the table without error
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
    /// Removes entries from a table within the specified key range, with support
    /// for selective filtering and pruning limits. This method provides fine-grained
    /// control over which entries are deleted through a skip filter function.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The table type to prune entries from
    ///
    /// # Parameters
    ///
    /// * `keys` - The range of keys to consider for deletion (inclusive/exclusive bounds supported)
    /// * `limiter` - Pruning limiter that controls deletion rate and total count
    /// * `skip_filter` - Function that returns true for rows that should be skipped (not deleted)
    /// * `delete_callback` - Function called for each deleted row (useful for logging/tracking)
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// * `usize` - The number of entries that were successfully deleted
    /// * `bool` - Whether the entire range was processed (true) or pruning was stopped due to
    ///   limits (false)
    ///
    /// # Behavior
    ///
    /// - Walks through the specified key range in order
    /// - For each row, calls skip_filter to determine if it should be deleted
    /// - Respects pruning limits and stops when limits are reached
    /// - Calls delete_callback for each successfully deleted row
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
    /// Performs a single step in a range-based pruning operation, processing one entry
    /// from the walker. This method is designed for fine-grained control over pruning
    /// operations, particularly when coordinating pruning across multiple tables.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The table type being pruned
    ///
    /// # Parameters
    ///
    /// * `walker` - A mutable reference to the range walker that iterates through table entries
    /// * `limiter` - Pruning limiter for tracking deleted entries (used for callback only)
    /// * `skip_filter` - Function that returns true for rows that should be skipped
    /// * `delete_callback` - Function called for each deleted row
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - If the walker has no more entries to process (finished)
    /// * `Ok(false)` - If the walker has more entries that could be processed
    /// * `Err(DatabaseError)` - If there was a database access error
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
