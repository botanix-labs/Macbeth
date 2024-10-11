use std::io::Write;

use crate::database::{Db, Error};
use bitcoin::hashes::{sha256, Hash};

/// Get the merkle root of the UTXO set.
pub fn get_utxo_set_merkle_root(db: &Db) -> Result<Vec<u8>, Error> {
    let root = db.get_utxo_merkle_root()?;
    if let Some(root) = root {
        Ok(root[..].to_vec())
    } else {
        Ok(vec![0u8; 32])
    }
}

/// Get the merkle root of the tracked tx set.
pub fn get_tracked_tx_set_merkle_root(db: &Db) -> Result<Vec<u8>, Error> {
    let root = db.get_tracked_tx_merkle_root()?;
    if let Some(root) = root {
        Ok(root[..].to_vec())
    } else {
        Ok(vec![0u8; 32])
    }
}

/// Get the wallet state commitment.
pub fn get_wallet_state_commitment(db: &Db) -> Result<Vec<u8>, Error> {
    let utxo_root = get_utxo_set_merkle_root(db)?;
    let tracked_tx_root = get_tracked_tx_set_merkle_root(db)?;
    let mut engine = sha256::Hash::engine();
    engine.write(&utxo_root);
    engine.write(&tracked_tx_root);
    let hash = sha256::Hash::from_engine(engine);

    Ok(hash.to_byte_array().to_vec())
}
