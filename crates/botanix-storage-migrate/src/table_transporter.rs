use crate::report::MigrationReport;
use eyre::Context;
use reth_db::{mdbx::tx::Tx, TableViewer};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    table::Table,
    transaction::{DbTx, DbTxMut},
};

/// A utility for transporting (migrating) data between reth and botanix database tables.
///
/// The `TableTransporter` is responsible for transferring all entries from a source
/// table in the reth database to a destination table in the botanix database. After
/// successful migration, the source table is cleared.
pub(crate) struct TableTransporter<'a> {
    /// Write transaction for the source (reth) database
    reth_tx: &'a Tx<reth_db::mdbx::RW>,
    /// Write transaction for the destination (botanix) database
    botanix_tx: &'a Tx<reth_db::mdbx::RW>,
}

impl<'a> TableTransporter<'a> {
    /// Creates a new `TableTransporter` with the provided database transactions.
    ///
    /// # Arguments
    ///
    /// * `reth_tx` - A write transaction for the source (reth) database
    /// * `botanix_tx` - A write transaction for the destination (botanix) database
    ///
    /// # Returns
    ///
    /// A new `TableTransporter` instance that can be used to migrate data
    /// between the specified database transactions.
    pub(crate) const fn new(
        reth_tx: &'a Tx<reth_db::mdbx::RW>,
        botanix_tx: &'a Tx<reth_db::mdbx::RW>,
    ) -> Self {
        Self { reth_tx, botanix_tx }
    }

    /// Transfers all entries from a reth database table to a botanix database table.
    ///
    /// This method performs the following steps:
    /// 1. Counts the entries in the source (reth) table
    /// 2. If the table is empty, returns an empty report
    /// 3. Creates cursors for both source and destination tables
    /// 4. Copies all entries from source to destination
    /// 5. Verifies that the number of migrated entries matches the expected count
    /// 6. Clears the source table after successful migration
    /// 7. Returns a migration report with statistics
    ///
    /// # Arguments
    ///
    /// * `reth_db_table` - The source table in the reth database
    /// * `botanix_db_table` - The destination table in the botanix database
    ///
    /// # Returns
    ///
    /// A `MigrationReport` containing statistics about the migration operation,
    /// or an error if any step in the migration process fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The source or destination table cursors cannot be created
    /// - Reading entries from the source table fails
    /// - Writing entries to the destination table fails
    /// - The number of migrated entries doesn't match the expected count
    /// - Clearing the source table fails
    fn transport_table<T: Table>(&self) -> eyre::Result<MigrationReport> {
        let start_time = std::time::Instant::now();

        // Check if table exists in source database by counting entries
        let Ok(reth_table_keys_count) = self.reth_tx.entries::<T>() else {
            // Table doesn't exist or is empty, nothing to migrate
            return Ok(MigrationReport::default());
        };

        if reth_table_keys_count == 0 {
            // Table is empty, nothing to migrate
            return Ok(MigrationReport::default());
        }

        let mut reth_db_cursor = self
            .reth_tx
            .cursor_read::<T>()
            .wrap_err(format!("Failed to create reth db write cursor for table '{}'", T::NAME))?;

        let mut botanix_db_cursor = self.botanix_tx.cursor_write::<T>().wrap_err(format!(
            "Failed to create botanix db write cursor for table '{}'",
            T::NAME
        ))?;

        let mut migrated_count = 0;

        // Walk through all entries in the source table and copy to destination
        for result in
            reth_db_cursor.walk(None).wrap_err(format!("Failed to walk table '{}'", T::NAME))?
        {
            let (key, value) =
                result.wrap_err(format!("Failed to read entry from table '{}'", T::NAME))?;

            botanix_db_cursor
                .append(key, value)
                .wrap_err(format!("Failed to append entry to table '{}'", T::NAME))?;

            migrated_count += 1;
        }

        if reth_table_keys_count != migrated_count {
            return Err(eyre::eyre!(
                "Mismatch in migrated entries: expected {}, got {}",
                reth_table_keys_count,
                migrated_count
            ));
        }

        self.reth_tx.clear::<T>().wrap_err(format!("Failed to clear table '{}'", T::NAME))?;

        Ok(MigrationReport::new(migrated_count, start_time))
    }
}

impl TableViewer<MigrationReport> for TableTransporter<'_> {
    type Error = eyre::Error;

    fn view<T: Table>(&self) -> eyre::Result<MigrationReport> {
        self.transport_table::<T>()
    }
}
