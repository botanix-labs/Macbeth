use botanix_storage_migrate::migrate_botanix_tables;
use eyre;
use reth_db::{test_utils::create_test_rw_db, DatabaseEnv};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    table::Table,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::BlockNumber;

/// Helper function to populate test data in botanix-storage tables
fn populate_botanix_test_data(db: &DatabaseEnv) -> eyre::Result<()> {
    let tx = db.tx_mut()?;

    // Populate Chunks table
    let mut chunks_cursor = tx.cursor_write::<reth_db::tables::Chunks>()?;
    for i in 1..=3 {
        let chunk_id = i as u64;
        let chunk = reth_db::models::SnapshotChunk::new(
            i as u64,
            BlockNumber::from(i as u64),
            vec![i as u8; 100],
        );
        chunks_cursor.append(chunk_id, chunk)?;
    }

    // Populate BlockSnapshots table
    let mut block_snapshots_cursor = tx.cursor_write::<reth_db::tables::BlockSnapshots>()?;
    for i in 1..=3 {
        let block_number = BlockNumber::from(i as u64);
        let snapshot_id = i as u64;
        block_snapshots_cursor.append(block_number, snapshot_id)?;
    }

    // Populate ChunkBlocks table
    let mut chunk_blocks_cursor = tx.cursor_write::<reth_db::tables::ChunkBlocks>()?;
    for i in 1..=3 {
        let chunk_id = i as u64;
        let block_number = BlockNumber::from(i as u64 * 10);
        chunk_blocks_cursor.append(chunk_id, block_number)?;
    }

    tx.commit()?;
    Ok(())
}

/// Helper function to populate test data in regular reth tables
fn populate_reth_test_data(db: &DatabaseEnv) -> eyre::Result<()> {
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
fn count_table_entries<T: Table>(db: &DatabaseEnv) -> eyre::Result<usize> {
    let tx = db.tx()?;
    let count = tx.entries::<T>().unwrap_or_else(|_| 0);
    Ok(count)
}

#[test]
fn test_migration() -> eyre::Result<()> {
    // Create temporary databases
    let reth_temp_db = create_test_rw_db();
    let reth_db = reth_temp_db.db();
    let botanix_temp_db = create_test_rw_db();
    let botanix_db = botanix_temp_db.db();

    // Populate test data in reth database
    populate_botanix_test_data(&reth_db)?;
    populate_reth_test_data(&reth_db)?;

    // Verify initial state

    // Botanix and reth tables have data in reth db
    assert_eq!(count_table_entries::<reth_db::tables::Chunks>(&reth_db)?, 3);
    assert_eq!(count_table_entries::<reth_db::tables::BlockSnapshots>(&reth_db)?, 3);
    assert_eq!(count_table_entries::<reth_db::tables::ChunkBlocks>(&reth_db)?, 3);
    assert_eq!(count_table_entries::<reth_db::tables::Headers>(&reth_db)?, 5);

    // Botanix db should be empty
    assert_eq!(count_table_entries::<botanix_storage::tables::Chunks>(&botanix_db)?, 0);
    assert_eq!(count_table_entries::<botanix_storage::tables::BlockSnapshots>(&botanix_db)?, 0);
    assert_eq!(count_table_entries::<botanix_storage::tables::ChunkBlocks>(&botanix_db)?, 0);

    // Perform migration
    migrate_botanix_tables(reth_db, botanix_db)?;

    // Verify migration results

    // Botanix tables in reth db should be empty
    assert_eq!(count_table_entries::<reth_db::tables::Chunks>(&reth_db)?, 0);
    assert_eq!(count_table_entries::<reth_db::tables::BlockSnapshots>(&reth_db)?, 0);
    assert_eq!(count_table_entries::<reth_db::tables::ChunkBlocks>(&reth_db)?, 0);

    // Regular reth tables should still have data
    assert_eq!(count_table_entries::<reth_db::tables::Headers>(&reth_db)?, 5);

    // Botanix db should have all migrated data
    assert_eq!(count_table_entries::<botanix_storage::tables::Chunks>(&botanix_db)?, 3);
    assert_eq!(count_table_entries::<botanix_storage::tables::BlockSnapshots>(&botanix_db)?, 3);
    assert_eq!(count_table_entries::<botanix_storage::tables::ChunkBlocks>(&botanix_db)?, 3);

    // Verify data integrity by checking specific entries
    let botanix_tx = botanix_db.tx()?;

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
