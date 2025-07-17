use crate::table_transporter::TableTransporter;
use botanix_storage::tables::Tables;
use eyre::Context;
use reth_db::{Database, DatabaseEnv};
use reth_db_api::transaction::DbTx;
use std::path::Path;

/// Checks if a migration from a reth database to a botanix database is needed.
///
/// This function verifies reth and botanix database paths and files inside.
/// If the reth database path exists and contains content, and the botanix database path
/// does not exist or is empty, it indicates that a migration is needed.
///
/// # Arguments
/// * `reth_db_path` - The path to the reth database directory
/// * `botanix_db_path` - The path to the botanix database directory
///
/// # Returns
/// Returns `Ok(true)` If migration is needed, `Ok(false)` if not
///
/// # Errors
///
/// This function will return an error if:
/// * The reth database path does not exist or is not a directory
/// * The botanix database path exists but is not a directory
/// * Reading the directory entries fails
pub fn is_migration_needed(reth_db_path: &Path, botanix_db_path: &Path) -> eyre::Result<bool> {
    let is_migration_needed =
        path_has_content(reth_db_path)? && !path_has_content(botanix_db_path)?;

    Ok(is_migration_needed)
}

fn path_has_content(path: &Path) -> eyre::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let entries = path.read_dir().wrap_err("Failed to read directory")?;

    Ok(entries.count() > 0)
}

/// Migrates botanix-storage tables from a reth database to a botanix database.
///
/// This function moves all data from the botanix-storage specific tables in the source
/// reth database to the corresponding tables in the destination botanix database.
/// The source tables are cleared after successful migration, making this a move operation.
///
/// The migrated tables are:
/// - Snapshots
/// - WalletStateSyncs
/// - StagedHeader
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
    let start_time = std::time::Instant::now();

    // Open mutable transactions for both databases

    let reth_tx =
        reth_db.tx_mut().wrap_err("Failed to create write transaction for reth database")?;

    let botanix_tx =
        botanix_db.tx_mut().wrap_err("Failed to create write transaction for botanix database")?;

    let transporter = TableTransporter::new(&reth_tx, &botanix_tx);

    tracing::info!("Migrating botanix tables from reth to botanix database...");

    let mut migrated_tables_count = 0;
    let mut elapsed_time = std::time::Duration::ZERO;

    for table in Tables::ALL {
        tracing::info!("Migrating table {}...", table.name());

        // Migrate the table and receive a report
        let report = table
            .view(&transporter)
            .wrap_err(format!("Failed to migrate {} table", table.name()))?;

        if report.is_migrated() {
            tracing::info!("Successfully migrated table {}: {}", table.name(), report);
            migrated_tables_count += 1;
            elapsed_time += report.elapsed_time;
        } else {
            tracing::info!("No entries to migrate for table {}. Skipping.", table.name());
        }
    }

    // Commit the transactions

    botanix_tx.commit().wrap_err("Failed to commit botanix transaction")?;
    reth_tx.commit().wrap_err("Failed to commit reth transaction")?;

    let skipped_tables_count = Tables::ALL.len() - migrated_tables_count;

    tracing::info!(
        "Migration completed successfully for {} tables ({} skipped) in {} secs",
        migrated_tables_count,
        skipped_tables_count,
        start_time.elapsed().as_secs_f64()
    );

    Ok(())
}
