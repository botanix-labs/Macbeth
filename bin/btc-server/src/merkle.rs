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
fn get_tracked_tx_set_merkle_root(db: &Db) -> Result<Vec<u8>, Error> {
    let root = db.get_tracked_tx_merkle_root()?;
    if let Some(root) = root {
        Ok(root[..].to_vec())
    } else {
        Ok(vec![0u8; 32])
    }
}

/// Get the merkle root of the pending pegouts set.
pub fn get_pending_pegouts_set_merkle_root(db: &Db) -> Result<Vec<u8>, Error> {
    let root = db.get_pending_pegouts_merkle_root()?;
    if let Some(root) = root {
        Ok(root[..].to_vec())
    } else {
        Ok(vec![0u8; 32])
    }
}

#[derive(Debug)]
pub struct WalletState {
    pub utxo_root: Vec<u8>,
    pub tracked_tx_root: Vec<u8>,
    pub pending_pegouts_root: Vec<u8>,
    pub wallet_state_commitment: Vec<u8>,
}

/// Get the wallet state commitment.
pub fn get_wallet_state_commitment(db: &Db) -> Result<WalletState, Error> {
    let utxo_root = get_utxo_set_merkle_root(db)?;
    let tracked_tx_root = get_tracked_tx_set_merkle_root(db)?;
    let pending_pegouts_root = get_pending_pegouts_set_merkle_root(db)?;
    let mut engine = sha256::Hash::engine();
    let _ = engine.write(&utxo_root).map_err(Error::HashEngine)?;
    let _ = engine.write(&tracked_tx_root).map_err(Error::HashEngine)?;
    let _ = engine.write(&pending_pegouts_root).map_err(Error::HashEngine)?;
    let hash = sha256::Hash::from_engine(engine);

    Ok(WalletState {
        utxo_root,
        tracked_tx_root,
        pending_pegouts_root,
        wallet_state_commitment: hash.to_byte_array().to_vec(),
    })
}
