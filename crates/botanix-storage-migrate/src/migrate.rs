use crate::table_transporter::TableTransporter;
use botanix_storage::tables::Tables;
use eyre::Context;
use reth_db::{Database, DatabaseEnv};
use reth_db_api::transaction::DbTx;

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

    tracing::info!("Migrate {} botanix tables from reth to botanix database", Tables::ALL.len());

    for table in Tables::ALL {
        tracing::info!("Migrating table {}...", table.name());

        // Migrate the table and receive a report
        let report = table
            .view(&transporter)
            .wrap_err(format!("Failed to migrate {} table", table.name()))?;

        tracing::info!("Successfully migrated table {}: {}", table.name(), report);
    }

    // Commit the transactions

    botanix_tx.commit().context("failed to commit botanix transaction")?;
    reth_tx.commit().wrap_err("Failed to commit reth transaction")?;

    tracing::info!(
        "Migration completed successfully for {} tables in {} secs",
        Tables::ALL.len(),
        start_time.elapsed().as_secs_f64()
    );

    Ok(())
}
