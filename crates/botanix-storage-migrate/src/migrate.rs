use botanix_storage::tables::Tables;
use eyre::Context;
use reth_db::{mdbx::tx::Tx, Database, DatabaseEnv, TableViewer};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    table::Table,
    transaction::{DbTx, DbTxMut},
};

/// Migrates botanix-storage tables from a reth database to a botanix database.
///
/// This function moves all data from the botanix-storage specific tables in the source
/// reth database to the corresponding tables in the destination botanix database.
/// The source tables are cleared after successful migration, making this a move operation.
///
/// The migrated tables are:
/// - Snapshots
/// - WalletStateSyncs
/// - StagedHeaders
/// - Chunks
/// - BlockSnapshots
/// - ChunkBlocks
/// - SnapshotSyncs
///
/// # Arguments
///
/// * `reth_db` - The source reth database environment
/// * `botanix_db` - The destination botanix database environment
///
/// # Returns
///
/// Returns `Ok(())` if the migration completes successfully, or an error if any step fails.
///
/// # Example
///
/// ```rust
/// use botanix_storage_migrate::migrate_botanix_tables;
/// use reth_db::{test_utils::create_test_rw_db, DatabaseEnv};
/// use std::{path::Path, sync::Arc};
///
/// let reth_db = create_test_rw_db();
/// let botanix_db = create_test_rw_db();
///
/// migrate_botanix_tables(reth_db.db(), botanix_db.db())?;
/// # Ok::<(), eyre::Error>(())
/// ```
pub fn migrate_botanix_tables(reth_db: &DatabaseEnv, botanix_db: &DatabaseEnv) -> eyre::Result<()> {
    let reth_tx =
        reth_db.tx_mut().wrap_err("Failed to create write transaction for reth database")?;

    let botanix_tx =
        botanix_db.tx_mut().wrap_err("Failed to create write transaction for botanix database")?;

    tracing::info!("Migrate {} botanix tables from reth to botanix database", Tables::ALL.len());

    for table in Tables::ALL {
        tracing::info!("Migrating table {}...", table.name());

        let migrator = TableMigrator { reth_tx: &reth_tx, botanix_tx: &botanix_tx };

        let report =
            table.view(&migrator).wrap_err(format!("Failed to migrate {} table", table.name()))?;

        tracing::info!(?report, "Successfully migrated table {}", table.name());
    }

    botanix_tx.commit().context("failed to commit botanix transaction")?;
    reth_tx.commit().wrap_err("Failed to commit reth transaction")?;

    tracing::info!("Migration completed successfully for {} tables", Tables::ALL.len());

    Ok(())
}

/// An intermediary struct to get generic from the tables enum
struct TableMigrator<'a> {
    reth_tx: &'a Tx<reth_db::mdbx::RW>,
    botanix_tx: &'a Tx<reth_db::mdbx::RW>,
}

impl TableViewer<MigrationReport> for TableMigrator<'_> {
    type Error = eyre::Error;

    fn view<T: Table>(&self) -> Result<MigrationReport, Self::Error> {
        migrate_table::<T>(self.reth_tx, self.botanix_tx)
    }
}

#[derive(Debug, Default)]
struct MigrationReport {
    migrated_count: usize,
    cleared_count: usize,
    elapsed_time: std::time::Duration,
}

impl MigrationReport {
    fn new(migrated_count: usize, cleared_count: usize, start_time: std::time::Instant) -> Self {
        Self { migrated_count, cleared_count, elapsed_time: start_time.elapsed() }
    }
}

/// Generic function to migrate data from one table type to another and clear the source.
fn migrate_table<T: Table>(
    reth_tx: &Tx<reth_db::mdbx::RW>,
    botanix_tx: &Tx<reth_db::mdbx::RW>,
) -> eyre::Result<MigrationReport> {
    let start_time = std::time::Instant::now();

    // Check if table exists in source database by counting entries
    let Ok(source_count) = reth_tx.entries::<T>() else {
        // Table doesn't exist or is empty, nothing to migrate
        return Ok(MigrationReport::default());
    };

    if source_count == 0 {
        // Table is empty, nothing to migrate
        return Ok(MigrationReport::default());
    }

    let mut source_cursor = reth_tx
        .cursor_read::<T>()
        .wrap_err(format!("Failed to create read cursor for table '{}'", T::NAME))?;
    let mut dest_cursor = botanix_tx
        .cursor_write::<T>()
        .wrap_err(format!("Failed to create write cursor for table '{}'", T::NAME))?;

    let mut migrated_count = 0;

    // Walk through all entries in the source table and copy to destination
    for result in
        source_cursor.walk(None).wrap_err(format!("Failed to walk table '{}'", T::NAME))?
    {
        let (key, value) =
            result.wrap_err(format!("Failed to read entry from table '{}'", T::NAME))?;

        dest_cursor
            .append(key, value)
            .wrap_err(format!("Failed to append entry to table '{}'", T::NAME))?;

        migrated_count += 1;
    }

    // Clear the source table after a successful migration
    let cleared_count = reth_tx
        .entries::<T>()
        .wrap_err(format!("Failed to count entries in table '{}'", T::NAME))?;

    reth_tx.clear::<T>().wrap_err(format!("Failed to clear table '{}'", T::NAME))?;

    Ok(MigrationReport::new(migrated_count, cleared_count, start_time))
}
