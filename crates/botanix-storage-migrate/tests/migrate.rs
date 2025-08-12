//! Test migration of botanix storage tables to reth database

use botanix_storage_migrate::{
    migrate_botanix_tables,
    test_utils::fixtures::{
        create_test_block_snapshots, create_test_chunk_blocks, create_test_chunks,
        create_test_runtime_transitions, create_test_snapshot_syncs, create_test_snapshots,
        create_test_staged_headers, create_test_wallet_state_syncs,
    },
    TABLES_TO_MIGRATE,
};
use eyre::Context;
use reth_db::{test_utils::create_test_rw_db, DatabaseEnv};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    table::Table,
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_primitives::BlockNumber;

/// Helper function to populate test data in botanix-storage tables
pub fn populate_botanix_test_data(db: &DatabaseEnv) -> eyre::Result<()> {
    let tx = db.tx_mut()?;

    let count = 3; // Number of entries to insert in each table

    // Insert minimal test data for each table
    for table in reth_db::Tables::ALL {
        match table {
            reth_db::Tables::Snapshots => {
                create_test_snapshots(&tx, count)?;
            }
            reth_db::Tables::WalletStateSyncs => {
                create_test_wallet_state_syncs(&tx, count)?;
            }
            reth_db::Tables::StagedHeader => {
                create_test_staged_headers(&tx, count)?;
            }
            reth_db::Tables::Chunks => {
                create_test_chunks(&tx, count)?;
            }
            reth_db::Tables::BlockSnapshots => {
                create_test_block_snapshots(&tx, count)?;
            }
            reth_db::Tables::ChunkBlocks => {
                create_test_chunk_blocks(&tx, count)?;
            }
            reth_db::Tables::SnapshotSyncs => {
                create_test_snapshot_syncs(&tx, count)?;
            }
            reth_db::Tables::RuntimeTransitions => {
                create_test_runtime_transitions(&tx, count)?;
            }
            _ => {} // Skip other tables
        }
    }

    // The tables are now populated using the helper functions in the match statement above

    tx.commit()?;

    Ok(())
}

/// Helper function to populate test data in regular reth tables
pub fn populate_reth_test_data(db: &DatabaseEnv) -> eyre::Result<()> {
    let tx = db.tx_mut()?;

    // Populate Headers table (regular reth table)
    let mut headers_cursor = tx.cursor_write::<reth_db::tables::Headers>()?;

    for i in 1..=5 {
        let block_number = BlockNumber::from(i as u64);
        let header = reth_primitives::Header {
            number: i as u64,
            gas_limit: 8000000,
            gas_used: 0,
            timestamp: 1000000 + i as u64,
            ..Default::default()
        };
        headers_cursor.append(block_number, header)?;
    }

    tx.commit()?;

    Ok(())
}

/// Helper function to count entries in a table
pub fn count_table_entries<T: Table>(db: &DatabaseEnv) -> eyre::Result<usize> {
    let tx = db.tx()?;

    let count = tx.entries::<T>().unwrap_or(0);

    Ok(count)
}

fn count_botanix_db_tables(db: &DatabaseEnv, expected_count: usize) -> eyre::Result<()> {
    use botanix_storage::tables::Tables;

    for table in TABLES_TO_MIGRATE {
        botanix_storage::tables_to_generic!(table, |Table| {
            assert_eq!(count_table_entries::<Table>(db)?, expected_count);
        });
    }

    Ok(())
}

fn count_reth_db_botanix_tables(db: &DatabaseEnv, expected_count: usize) -> eyre::Result<()> {
    use botanix_storage::tables::Tables;

    for table in TABLES_TO_MIGRATE {
        botanix_storage::tables_to_generic!(table, |Table| {
            // Count entries in the botanix table
            assert_eq!(count_table_entries::<Table>(db)?, expected_count);
        });
    }

    Ok(())
}

#[test]
fn test_migration() -> eyre::Result<()> {
    // Create temporary databases
    let reth_temp_db = create_test_rw_db();
    let reth_db = reth_temp_db.db();
    let botanix_temp_db = create_test_rw_db();
    let botanix_db = botanix_temp_db.db();

    // Populate test data in reth database
    populate_botanix_test_data(reth_db).wrap_err("failed to populate botanix test data")?;
    populate_reth_test_data(reth_db).wrap_err("failed to populate reth test data")?;

    // Verify initial state

    // Botanix and reth tables have data in reth db

    // Botanix db should be empty
    count_botanix_db_tables(botanix_db, 0).wrap_err("failed to count botanix tables")?;

    // Perform migration
    migrate_botanix_tables(reth_db, botanix_db).wrap_err("failed to migrate botanix tables")?;

    // Verify migration results

    // Botanix tables in reth db should be empty
    count_reth_db_botanix_tables(reth_db, 0)
        .wrap_err("failed to count reth botanix tables after migration")?;

    // Regular reth tables should still have data
    assert_eq!(count_table_entries::<reth_db::tables::Headers>(reth_db)?, 5);

    // Botanix db should have all migrated data
    count_botanix_db_tables(botanix_db, 3).wrap_err("failed to count botanix tables")?;

    // Verify data integrity by checking specific entries
    let botanix_tx = botanix_db.tx().wrap_err("failed to get tx")?;

    // Check Chunks data
    let mut chunks_cursor = botanix_tx.cursor_read::<botanix_storage::tables::Chunks>()?;
    let chunk_data = chunks_cursor.walk(None)?.collect::<Result<Vec<_>, _>>()?;

    assert_eq!(chunk_data.len(), 3);

    // Check that the first chunk has expected data
    let (chunk_id, chunk) = &chunk_data[0];
    assert_eq!(*chunk_id, 1);
    assert_eq!(chunk.get_starting_block_number(), 1);

    Ok(())
}
