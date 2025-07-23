//! This module is using deprecated reth_db_api traits and types to test migration

#![allow(deprecated)]

use reth_db_api::{
    cursor::DbCursorRW,
    models::{PeerID, UuidID},
    transaction::DbTxMut,
};
use reth_primitives::{BlockNumber, Header, B256};

/// Helper function to create test snapshots
pub fn create_test_snapshots<TX: DbTxMut>(tx: &TX, count: usize) -> eyre::Result<()> {
    let mut cursor = tx.cursor_write::<reth_db::tables::Snapshots>()?;

    for i in 1..=count {
        let snapshot_id = i as u64;
        let snapshot =
            reth_db::models::Snapshot::new(snapshot_id, snapshot_id * 100, B256::random());
        cursor.append(snapshot_id, snapshot)?;
    }

    Ok(())
}

/// Helper function to create test wallet state syncs
pub fn create_test_wallet_state_syncs<TX: DbTxMut>(tx: &TX, count: usize) -> eyre::Result<()> {
    // Generate block hashes and sort them to ensure proper append order
    let mut peer_ids: Vec<PeerID> = (1..=count).map(|_| PeerID::random()).collect();

    // Sort the hashes to ensure they are in the correct order for append
    peer_ids.sort();

    let mut cursor = tx.cursor_write::<reth_db::tables::WalletStateSyncs>()?;

    for (i, peer_id) in peer_ids.into_iter().enumerate() {
        let record =
            reth_db::models::WalletStateSyncRecord::new(peer_id, UuidID::random(), i as u64, None);
        cursor.append(peer_id, record)?;
    }

    Ok(())
}

/// Helper function to create test staged headers
pub fn create_test_staged_headers<TX: DbTxMut>(tx: &TX, count: usize) -> eyre::Result<()> {
    // Generate block hashes and sort them to ensure proper append order
    let mut block_hashes: Vec<B256> = (1..=count).map(|_| B256::random()).collect();

    // Sort the hashes to ensure they are in the correct order for append
    block_hashes.sort();

    let mut cursor = tx.cursor_write::<reth_db::tables::StagedHeader>()?;

    for (i, block_hash) in block_hashes.into_iter().enumerate() {
        let header = reth_db::models::HeaderWithPegs {
            header: Header {
                number: i as u64,
                gas_limit: 8000000,
                gas_used: 0,
                timestamp: 1000000 + i as u64,
                ..Default::default()
            },
            pegins: vec![],
            pegouts: vec![],
        };
        cursor.append(block_hash, header)?;
    }

    Ok(())
}

/// Helper function to create test chunks
pub fn create_test_chunks<TX: DbTxMut>(tx: &TX, count: usize) -> eyre::Result<()> {
    let mut cursor = tx.cursor_write::<reth_db::tables::Chunks>()?;

    for i in 1..=count {
        let chunk_id = i as u64;
        let chunk = reth_db::models::SnapshotChunk::new(
            i as u64,
            BlockNumber::from(i as u64),
            vec![i as u8; 100],
        );
        cursor.append(chunk_id, chunk)?;
    }

    Ok(())
}

/// Helper function to create test block snapshots
pub fn create_test_block_snapshots<TX: DbTxMut>(tx: &TX, count: usize) -> eyre::Result<()> {
    let mut cursor = tx.cursor_write::<reth_db::tables::BlockSnapshots>()?;

    for i in 1..=count {
        let block_number = BlockNumber::from(i as u64);
        let snapshot_id = i as u64;
        cursor.append(block_number, snapshot_id)?;
    }

    Ok(())
}

/// Helper function to create test chunk blocks
pub fn create_test_chunk_blocks<TX: DbTxMut>(tx: &TX, count: usize) -> eyre::Result<()> {
    let mut cursor = tx.cursor_write::<reth_db::tables::ChunkBlocks>()?;

    for i in 1..=count {
        let chunk_id = i as u64;
        let block_number = BlockNumber::from(i as u64 * 10);
        cursor.append(chunk_id, block_number)?;
    }

    Ok(())
}

/// Helper function to create test snapshot syncs
pub fn create_test_snapshot_syncs<TX: DbTxMut>(tx: &TX, count: usize) -> eyre::Result<()> {
    let mut cursor = tx.cursor_write::<reth_db::tables::SnapshotSyncs>()?;

    for i in 1..=count {
        let sync_id = i as u64;
        let sync = reth_db::models::SnapshotSync::new(i as u64, B256::random(), 3, i as u64);
        cursor.append(sync_id, sync)?;
    }

    Ok(())
}

/// Helper function to create test runtime transitions
pub fn create_test_runtime_transitions(tx: &impl DbTxMut, count: usize) -> eyre::Result<()> {
    let mut cursor = tx.cursor_write::<reth_db::tables::RuntimeTransitions>()?;

    for i in 1..=count {
        let height = i as u64;
        let transition = reth_db::models::RuntimeVersion::new(1, i as u16);
        cursor.append(height, transition)?;
    }

    Ok(())
}
