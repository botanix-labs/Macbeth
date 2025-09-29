/// This module is responsible for tracking pegout transactions and detecting when they are
/// confirmed or when they need to be retried.
/// Some vocab used in this file
/// - pending pegout: a pegout that is pending to be signed and broadcasted.
/// - tracked tx: a transaction that is confirmed but not sufficiently deep.
/// - confirmed tx: a transaction that has > 1 confirmation.
/// - finalized tx: a transaction that is deeply confirmed.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::{Duration, SystemTime},
};

use crate::{
    database::{FinalizedPegout, Utxo},
    pegout_id::PegoutId,
    telemetry::Telemetry,
    update_telemetry_error,
    wallet::{
        address::generate_taproot_change_scriptpubkey,
        util::{VerifyingKeyExt, VerifyingKeyExtError},
    },
};
use bitcoin::{Amount, Block, BlockHash, OutPoint, ScriptBuf, Transaction, TxOut, Txid};
use bitcoincore_rpc::RpcApi;
use log::{debug, error, info, trace, warn};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{database, rpc};

pub const TX_NOT_FOUND_BITCOIND_ERROR: &str = "no such mempool or blockchain transaction";
pub const TX_NOT_IN_MEMPOOL_BITCOIND_ERROR: &str = "transaction not in mempool";

macro_rules! print_safe {
    ($e:expr) => {
        $e.map(|v| v.to_string()).unwrap_or("ERR".to_owned())
    };
}

trait HeaderExt {
    fn block_timestamp(&self) -> u32;

    fn block_time(&self) -> SystemTime {
        let timestamp = self.block_timestamp();
        std::time::UNIX_EPOCH
            .checked_add(Duration::from_secs(timestamp as u64))
            .expect("u32 can't overflow unix time")
    }
}
impl HeaderExt for bitcoin::blockdata::block::Header {
    fn block_timestamp(&self) -> u32 {
        self.time
    }
}
impl HeaderExt for bitcoin::blockdata::block::Block {
    fn block_timestamp(&self) -> u32 {
        self.header.time
    }
}
impl HeaderExt for bitcoincore_rpc::json::GetBlockHeaderResult {
    fn block_timestamp(&self) -> u32 {
        self.time.try_into().expect("header timestamps are u32")
    }
}

/// Transaction with metadata about which outputs are pegouts and which are change.
/// This is used to track pegouts and detect when they are confirmed or when they need
/// to be retried.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tx {
    /// The transaction id on L1
    pub txid: Txid,
    /// The broadcasted transaction on L1
    pub tx: Transaction,
    /// Which indices in `tx.output` are pegouts
    pub pegout_idxs: Vec<usize>,
    /// List of pegout requests that this tx is delivering
    /// the size of this vec is equal to the length of pegout_idxs
    pub pegout_requests: Vec<PegoutRequest>,
    /// Which indices in `tx.output` are change back to the federation wallet
    pub change_idxs: Vec<usize>,
    /// When this transaction was created
    pub created: SystemTime,
}

impl Tx {
    pub fn inputs(&self) -> impl Iterator<Item = OutPoint> + '_ {
        self.tx.input.iter().map(|i| i.previous_output)
    }

    /// Get all the pegouts of this tx. These are the outputs this tx delivers.
    /// I.e. all outputs that are not change outputs.
    pub fn pegouts(&self) -> impl ExactSizeIterator<Item = (OutPoint, &TxOut)> + '_ {
        self.pegout_idxs.iter().map(|i| {
            let point = OutPoint::new(self.txid, *i as u32);
            let output = &self.tx.output[*i];
            (point, output)
        })
    }

    /// Get all change outputs of this tx.
    pub fn change(&self) -> impl ExactSizeIterator<Item = (OutPoint, &TxOut)> + '_ {
        self.change_idxs.iter().map(|i| {
            let point = OutPoint::new(self.txid, *i as u32);
            let output = &self.tx.output[*i];
            (point, output)
        })
    }
}

#[derive(Debug, Clone)]
struct BlockInfo {
    hash: BlockHash,
    relevant_txs: Vec<Txid>,
    relevant_inputs: Vec<OutPoint>,
}

/// A pegout request from the federation wallet to a user defined scriptpubkey.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PegoutRequest {
    /// A unique id to link back to L2 block
    pub id: PegoutId,
    /// The scriptpubkey of the pegout request (user destination).
    pub spk: bitcoin::ScriptBuf,
    /// The btc amount of pegout to deliver.
    pub value: Amount,
    /// L2 block height this pegout was requested at.
    pub botanix_height: u64,
}

impl PegoutRequest {
    pub fn txout(&self) -> TxOut {
        TxOut { script_pubkey: self.spk.clone(), value: self.value }
    }
}

impl TryFrom<rpc::PendingPegout> for PegoutRequest {
    type Error = tonic::Status;

    fn try_from(pegout: rpc::PendingPegout) -> Result<Self, Self::Error> {
        Ok(PegoutRequest {
            id: PegoutId::from_bytes(&pegout.pegout_id).expect("valid pegout id"),
            spk: ScriptBuf::from_bytes(pegout.spk),
            value: Amount::from_sat(pegout.amount),
            botanix_height: pegout.height,
        })
    }
}

#[allow(dead_code)]
/// A pegout that is pending to be signed and broadcasted.
struct PendingPegout {
    request: PegoutRequest,
    attempts: HashSet<bitcoin::Txid>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum OutputMeta {
    Pegout(PegoutId),
    Change,
}

impl OutputMeta {
    #[allow(dead_code)]
    pub fn is_pegout(&self) -> bool {
        match self {
            Self::Pegout(_) => true,
            Self::Change => false,
        }
    }

    #[allow(dead_code)]
    pub fn is_change(&self) -> bool {
        match self {
            Self::Pegout(_) => false,
            Self::Change => true,
        }
    }

    #[allow(dead_code)]
    pub fn pegout_id(&self) -> Option<PegoutId> {
        match self {
            Self::Pegout(id) => Some(*id),
            Self::Change => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ChangeOutputError {
    #[error("key conversion error {0}")]
    KeyConversion(#[from] VerifyingKeyExtError),
    #[error("db error {0}")]
    Db(#[from] database::Error),
}

pub struct PegoutScheduler {
    /// The number of blocks to track txs for.
    conf_window: u32,
    /// The set of txs we are tracking.
    /// The purpose of tracking the txs is to detect when they have
    /// sufficiently deep confirmations on L1. Once they do change outputs may
    /// be added to the UTXO set as a spendable output
    /// If a tracked tx is reorged or dropped from the mempool the application must
    /// Add the non-change outputs back to the pending pegouts set.
    txs: HashMap<Txid, Tx>,
    /// A mapping of input to the txs that spend them.
    /// This is used to detect when a tracked tx is reorged or dropped from the mempool.
    txs_by_input: HashMap<OutPoint, Vec<Txid>>,
    /// A mapping of pegout output to the txs that produce them.
    txs_by_pegout: HashMap<TxOut, Vec<Txid>>,
    /// The txs that are confirmed but not finalized yet.
    confirmed_txs: HashSet<Txid>,
    /// The last [conf_window] blocks we have seen. This data structure
    /// includes txs and inputs that are relevant to the txs we are tracking.
    last_blocks: VecDeque<BlockInfo>,
    /// The last block that was finalized.
    last_finalized: BlockHash,
    /// Database handle
    db: database::Db,
    /// Whether we should scan for missing change outputs
    scan_for_change_outputs: bool,
}

impl PegoutScheduler {
    pub fn new(
        conf_window: u32,
        txs: Vec<Tx>,
        last_finalized: BlockHash,
        db: database::Db,
    ) -> PegoutScheduler {
        let mut ret = PegoutScheduler {
            conf_window,
            txs: HashMap::with_capacity(txs.len()),
            txs_by_input: HashMap::with_capacity(txs.iter().map(|t| t.tx.input.len()).sum()),
            txs_by_pegout: HashMap::with_capacity(txs.iter().map(|t| t.pegouts().len()).sum()),
            confirmed_txs: HashSet::new(),
            last_blocks: VecDeque::with_capacity(conf_window as usize),
            last_finalized,
            db,
            scan_for_change_outputs: false,
        };

        ret.last_blocks.push_back(BlockInfo {
            hash: last_finalized,
            relevant_txs: Vec::new(),
            relevant_inputs: Vec::new(),
        });

        for tx in txs {
            ret.track_tx(tx);
        }

        ret
    }

    /// internal util to get change spk from db
    fn get_change_spk(&self) -> Result<ScriptBuf, ChangeOutputError> {
        let agg_pk = self
            .db
            .get_public_key_package()?
            .expect("pk key package should exist")
            .verifying_key()
            .to_secp_pk()?;
        let change_spk = generate_taproot_change_scriptpubkey(&agg_pk);
        Ok(change_spk)
    }

    /// Get the last finalized block hash.
    pub fn last_finalized(&self) -> BlockHash {
        self.last_finalized
    }

    /// Track a new transaction if it's not already tracked.
    fn track_tx(&mut self, tx: Tx) {
        for input in tx.inputs() {
            self.txs_by_input.entry(input).or_default().push(tx.txid);
        }
        for (_utxo, pegout) in tx.pegouts() {
            self.txs_by_pegout.entry(pegout.clone()).or_default().push(tx.txid);
        }
        self.txs.insert(tx.txid, tx);
    }

    /// Add a new tx to the index for tracking.
    /// Most of the work is being done in [Self::track_tx].
    /// This should be called when a new pegout transaction is broadcasted on L1.
    ///
    /// Panics if [pegouts] isn't a strict subset of the transaction's outputs.
    pub fn add_tx(
        &mut self,
        tx: Transaction,
        pegouts: &[PegoutRequest],
        timestamp: SystemTime,
    ) -> &Tx {
        let pegout_idxs = {
            let mut ret = Vec::with_capacity(pegouts.len());
            info!(
                "PegoutScheduler::add_tx: tx output length: {}, pegouts length: {}",
                tx.output.len(),
                pegouts.len()
            );
            // TODO: the same pegout could be in the tx multiple times
            for pegout in pegouts {
                let pegout_txout = pegout.txout();
                let idx = tx
                    .output
                    .iter()
                    .position(|txout| txout.script_pubkey == pegout_txout.script_pubkey)
                    .expect("tx doesn't contain all pegouts");
                ret.push(idx);
            }
            ret
        };
        let change_idxs = {
            let mut ret = Vec::with_capacity(pegout_idxs.len());
            for (i, txout) in tx.output.iter().enumerate() {
                if pegout_idxs.contains(&i) {
                    continue;
                }
                // sanity check that the change output spk is correct
                if txout.script_pubkey != self.get_change_spk().expect("change spk should exist") {
                    warn!(
                        "PegoutScheduler::add_tx: Change output spk in tx {} is not correct: {:?}",
                        tx.compute_txid(),
                        txout.script_pubkey
                    );
                    continue;
                }
                ret.push(i);
            }
            ret
        };
        let txid = tx.compute_txid();
        info!(
            "PegoutScheduler::add_tx: Tracking txid={}, pegout_idxs={:?}, change_idxs={:?}",
            txid, pegout_idxs, change_idxs
        );
        self.track_tx(Tx {
            created: timestamp,
            change_idxs,
            txid,
            tx,
            pegout_idxs,
            pegout_requests: pegouts.to_vec(),
        });
        self.txs.get(&txid).expect("just put it in")
    }

    /// Get all tracked tx pegout request ids.
    /// This is used by the coordinator to determine if it is retrying pegouts
    /// so it can add a conflicting input to the tx it is creating.
    pub fn tracked_pegout_request_ids(&self) -> Vec<PegoutId> {
        self.txs
            .values()
            .flat_map(|tx| tx.pegout_requests.clone().into_iter().map(|p| p.id))
            .collect::<Vec<_>>()
    }

    /// Get all input utxos that are spent by tracked txs.
    /// This is used by the coordinator to create pegouts that conflict with the inputs of tracked
    /// txs.
    pub fn tracked_inputs(&self) -> HashSet<OutPoint> {
        let mut ret = HashSet::with_capacity(self.txs.len() * 3);
        for tx in self.txs.values() {
            ret.extend(tx.inputs());
        }
        ret.shrink_to_fit();
        ret
    }

    /// Get all utxos that are created by tracked txs but are already confirmed.
    #[allow(dead_code)]
    pub fn pending_confirmed_utxos(&self) -> HashSet<OutPoint> {
        let mut ret = HashSet::with_capacity(self.txs.len() * 3);
        for tx in self.txs.values() {
            if self.confirmed_txs.contains(&tx.txid) {
                for vout in 0..tx.tx.output.len() {
                    ret.insert(OutPoint::new(tx.txid, vout as u32));
                }
            }
        }
        ret.shrink_to_fit();
        ret
    }

    /// Remove a tx from the tracked set.
    /// This should be called when a tracked tx is reorged or dropped from the mempool.
    /// Its expected the caller will add the pegout outputs back to the pending pegout set. This is
    /// not done by this function Note: will panic if provided txid is not tracked
    /// Note: Technically we should not untrack a tx simply because it was dropped from our local
    /// mempool, as it may still be in other nodes mempools and could still get confirmed.
    /// Leaving this as-is for now as it'll be resolved with the new TEM implementation.
    fn un_track_tx(&mut self, txid: &Txid) -> Result<(), database::Error> {
        let tx = self.txs.get(txid).expect("relevant tx should exist");
        info!("PegoutScheduler::un_track_tx: Untracking txid={}", txid);
        for input in tx.inputs() {
            self.txs_by_input.remove(&input);
        }
        for (_utxo, txout) in tx.pegouts() {
            self.txs_by_pegout.remove(txout);
        }
        self.txs.remove(txid);
        // Need to remove from the database as well
        self.db.remove_tracked_tx(txid)?;
        Ok(())
    }

    /// Add a tx back into the pending pegout set
    /// This should be called when a tracked tx is reorged or dropped from the mempool.
    /// Its expected the caller will remove the tx from the tracked set
    fn add_tx_back_to_pending(&mut self, tx: &Tx) -> Result<(), database::Error> {
        let pegout_refs: Vec<&PegoutRequest> = tx.pegout_requests.iter().collect();
        self.db.store_pending_pegouts(&pegout_refs)?;

        Ok(())
    }

    fn rollback_tip(&mut self) {
        assert!(!self.last_blocks.is_empty());
        let drop = self.last_blocks.pop_back().unwrap();
        info!(
            "PegoutScheduler::rollback_tip: Rolling back block {}, relevant_txs={:?}, relevant_inputs={:?}",
            drop.hash,
            drop.relevant_txs,
            drop.relevant_inputs
        );
        for txid in drop.relevant_txs {
            let tx = self.txs.get(&txid).expect("relevant tx should exist").clone();
            // Currently confirmed_txs is not used, could remove this
            self.confirmed_txs.remove(&txid);
            // TODO should we remove the expect here
            self.un_track_tx(&txid).expect("untrack tx");
            self.add_tx_back_to_pending(&tx).expect("add tx back to pending");
        }
    }

    /// Finalize a block by adding the UTXOs that are deeply confirmed back to the database.
    /// This is also where we remove tracked transactions
    fn finalize_block(&mut self, block: &BlockInfo) -> Result<(), database::Error> {
        info!("PegoutScheduler::finalize_block: Finalizing block {}", block.hash);
        let change_spk_res = self.get_change_spk(); // Get result first
        info!("PegoutScheduler::finalize_block: Expected change SPK result: {:?}", change_spk_res);

        // To make sure we only update the index when the db is also synced,
        // first try store the new finalized UTXOs to the db, then update the index.
        let mut all_inputs = block.relevant_inputs.iter().copied().collect::<HashSet<_>>();
        for txid in &block.relevant_txs {
            let tx =
                self.txs.get(txid).ok_or(database::Error::TrackedTxNotFoundInPegoutScheduler)?;
            // Add back the change to the utxo set
            let mut change_utxos = vec![];
            if let Ok(ref change_spk) = change_spk_res {
                // Check if we got the SPK successfully
                for (outpoint, output) in tx.change() {
                    if &output.script_pubkey != change_spk {
                        warn!(
                            "Finalizing block {}: Change output in tx {} being tracked is not the expected p2tr: {:?} != {:?}",
                            block.hash,
                            txid,
                            output.script_pubkey,
                            change_spk
                        );
                        continue;
                    }
                    let utxo_version = self
                        .db
                        .get_utxo(outpoint)
                        .ok()
                        .flatten()
                        .map(|utxo| utxo.version)
                        .unwrap_or_default();
                    change_utxos.push(database::Utxo {
                        outpoint,
                        output: output.clone(),
                        eth_address: None,
                        version: utxo_version,
                    });
                }
            } else {
                // Log if we couldn't get the expected change SPK
                error!(
                    "Finalizing block {}: Could not get expected change SPK to verify change outputs for tx {}. Error: {:?}",
                    block.hash,
                    txid,
                    change_spk_res.as_ref().err()
                );
            }
            self.db.store_utxos(change_utxos.iter().collect::<Vec<_>>().as_slice())?;
            self.db.flush()?;
            all_inputs.extend(tx.tx.input.iter().map(|i| i.previous_output));
        }

        // Now that it's all in the db, we can apply changes here.
        info!("PegoutScheduler::finalize_block: Processing finalized inputs: {:?}", all_inputs);
        for input in all_inputs {
            // Remove the tracked tx from the database as well
            // NOTE: This removes based on input.txid, which might be a conflicting tx, not the one
            // in the block
            if let Some(_tx) = self.txs.get(&input.txid).cloned() {
                // This input conflicts with a tx we were tracking. That tracked tx is now
                // definitely dead. Its pegouts failed. We should *not* mark them as
                // finalized. We already removed the UTXO (`input`) from the DB.
                // We just need to remove the dead tx from tracking.
                info!("Dropping tracked tx {} because its input {} was spent by another tx in finalized block {}", input.txid, input, block.hash);
                self.txs.remove(&input.txid);
                self.db.remove_tracked_tx(&input.txid)?;
                // We *could* add the pegouts back to pending, but likely the conflicting tx already
                // paid them. Let L2 handle potential duplicates if necessary.
            }
            // Remove the spent input UTXO if it exists in our DB (it might not if it wasn't ours)
            self.db.remove_utxo(&input)?;
            self.db.flush()?;
        }

        // Process transactions confirmed in this finalized block
        for txid in &block.relevant_txs {
            // Retrieve the Tx object before removing it
            if let Some(tx) = self.txs.get(txid).cloned() {
                let finalized_pegout_ids: Vec<FinalizedPegout> = tx
                    .pegout_requests
                    .iter()
                    .map(|pegout_request| FinalizedPegout {
                        id: pegout_request.id,
                        block_number: pegout_request.botanix_height,
                    })
                    .collect();

                if !finalized_pegout_ids.is_empty() {
                    info!(
                        "Storing {} finalized pegout IDs for confirmed tx {}",
                        finalized_pegout_ids.len(),
                        txid
                    );
                    let refs: Vec<&FinalizedPegout> = finalized_pegout_ids.iter().collect();
                    self.db.store_finalized_pegout_ids_atomically(&refs)?;
                    self.db.flush()?;
                } else {
                    info!("Confirmed tx {} had no associated pegout requests to finalize.", txid);
                }

                // Now remove the finalized tx from tracking
                self.txs.remove(txid);
                self.db.remove_tracked_tx(txid)?;
            } else {
                // This case should ideally not happen if relevant_txs is derived correctly
                warn!("Txid {} marked as relevant in finalized block {}, but not found in tracked txs.", txid, block.hash);
                // Attempt removal from DB just in case it's only present there
                self.db.remove_tracked_tx(txid)?;
            }
        }

        self.last_finalized = block.hash;
        self.db.flush()?;

        Ok(())
    }

    /// Adds a new block to the chain.
    ///
    /// Updates the [SyncResult] with the data from newly finalized blocks.
    fn add_block(&mut self, block: &Block, height: usize) {
        let hash: BlockHash = block.block_hash();

        // TODO this is broken for certain block heights https://github.com/rust-bitcoin/rust-bitcoin/issues/3583
        // let height = block.bip34_block_height().map_err(|e| {
        //     error!("bip34 is not active: {:?}", e);
        //     panic!("bip34 is not active: {:?}", e);
        // }).expect("bip34 is active");
        let last = self.last_blocks.back().expect("always something");
        assert_eq!(block.header.prev_blockhash, last.hash, "adding {}:{}", height, hash);

        let mut relevant_txs = Vec::new();
        let mut relevant_inputs = Vec::new();
        for tx in &block.txdata {
            let txid = tx.compute_txid();
            if self.txs.contains_key(&txid) {
                debug!("Indexed tx {} confirmed in block {}:{}", txid, height, hash);
                relevant_txs.push(txid);
                self.confirmed_txs.insert(txid);
            } else {
                for input in &tx.input {
                    if let Some(conflicts) = self.txs_by_input.get(&input.previous_output) {
                        warn!(
                            "Tx confirmed that conflicts with one of our txs: \
                            new={}, ours={:?}, block={}:{}",
                            txid, conflicts, height, hash,
                        );
                        // TODO future pegouts that use these inputs need to be retried
                        // TODO or check that does pegouts are being spent in this tx
                        // TODO also need to stop tracking the tx
                        // Only perform this logic after this BlockInfo is deeply confirmed
                        relevant_inputs.push(input.previous_output);
                    }
                }
            }
        }

        self.last_blocks.push_back(BlockInfo { hash, relevant_txs, relevant_inputs });
    }

    /// Check if tx is in the mempool, has been dropped or there was a reorg.
    pub fn track_mempool(
        &mut self,
        bitcoind: &impl RpcApi,
        checkpoint: bitcoincore_rpc::json::GetBlockHeaderResult,
    ) -> Result<(), SyncError> {
        // Determine the timestamp of the checkpoint block
        let cp_time = checkpoint.block_time();
        info!(
            "PegoutScheduler::track_mempool: Checking tracked txs older than checkpoint time {:?}",
            cp_time
        );
        // Get txs older than the checkpoint time
        let maybe_dropped_txs = self
            .txs
            .values()
            .filter(|tx| tx.created < cp_time)
            .map(|tx| tx.txid)
            .collect::<Vec<_>>();
        debug!(
            "Txids older than checkpoint: {:?}",
            maybe_dropped_txs.iter().map(|t| t.to_string()).collect::<Vec<_>>()
        );

        // Check if tx still exists
        for txid in maybe_dropped_txs {
            // Check if the tx is on chain and is actually deeply confirmed.
            // We use the tracked_tx.created timestamp but we don't know when the tx was actually
            // included in a block.
            let onchain_tx = bitcoind.get_raw_transaction_info(&txid, None);
            if let Ok(onchain_tx) = onchain_tx {
                // Check if the tx is deeply confirmed
                let Some(blockhash) = onchain_tx.blockhash else {
                    // Still in the mempool since blockhash is None
                    info!("PegoutScheduler::track_mempool: Tx {} is still in the mempool.", txid);
                    continue;
                };
                let actual_height =
                    bitcoind.get_block_info(&blockhash).map_err(SyncError::Rpc)?.height;
                if actual_height > checkpoint.height {
                    info!("Tx {} is confirmed in block {} at height {}, but is not deeply confirmed (checkpoint height {}).",
                        txid, blockhash, actual_height, checkpoint.height);
                    // Continue so sync_until will finalize the block and tracked tx once it is
                    // deeply confirmed
                    continue;
                }

                // This should not happen if sync_until functions correctly
                error!(
                    "PegoutScheduler::track_mempool: {:?}.",
                    SyncError::DeeplyConfirmedTxNotFinalized(txid)
                );
                // Intentionally not untracking the tx here since this should not happen.
                // If it does, the underlying issue should be fixed and the tracked tx handled.
                continue;
            }

            // a tx that is in a deeply confirmed block should have been handled already
            // so check if still in mempool
            let tx = self.txs.get(&txid).expect("tx should exist").clone();
            match bitcoind.get_mempool_entry(&txid) {
                Ok(_) => {
                    warn!("Tx {} still in the mempool", &txid);
                    // nothing else to do: eventually the tx will be confirmed or dropped
                    continue;
                }
                Err(e) => {
                    info!(
                        "PegoutScheduler::track_mempool: Tx {} not found in mempool (Error: {}).",
                        txid, e
                    );
                    // check error message to confirm the tx is not in the mempool
                    if !e.to_string().to_lowercase().contains(TX_NOT_IN_MEMPOOL_BITCOIND_ERROR) {
                        warn!("Error checking mempool for tx {}: {}", &txid, e);
                        continue;
                    }

                    // the tx has been dropped or there was a reorg
                    info!("PegoutScheduler::track_mempool: Tx {} confirmed not on chain. Untracking and adding back to pending.", txid);
                    self.un_track_tx(&txid)?;
                }
            }

            // add the tx back to pending pegouts so it can be retried
            // validate_psbt() will enforce the retry tx will have a conflicting input
            // so multiple outputs for the pegout are not created
            self.add_tx_back_to_pending(&tx)?;
            info!("Adding tx back to pending pegouts: {:?}", tx);
        }

        Ok(())
    }

    pub fn change_utxos(
        &self,
    ) -> Result<Vec<bitcoincore_rpc::json::Utxo>, Box<dyn std::error::Error>> {
        let json_data = r#"[{
      "txid": "d8b268a579ffbc5e425d69ef5f7e0f1c8db8c73b6b13b6f5a06caf4788129705",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00180655,
      "height": 903664
    },
    {
      "txid": "2792d9c79713b7b3d2c1d0267ec567a9e05f79b18355c782f379b35ca08bd50d",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.01088904,
      "height": 904468
    },
    {
      "txid": "365e926b53fd9c01bda2d44b4ce2fd04eb97c63fffc732c8813cb7f0e625c40e",
      "vout": 2,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00146289,
      "height": 904173
    },
    {
      "txid": "96de65caabd74b37d20af813900982692634aed9d7f791f37a71f504cbd91c10",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00005919,
      "height": 904336
    },
    {
      "txid": "7a5816ae0b9bfa77bd370e2af034737823107038ac96ce125b2d2aec95ff3e13",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00088026,
      "height": 898617
    },
    {
      "txid": "13850e99025ab1939d5e4bae8a2f11c33dd9001246fa783a74aefd2193166518",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00002113,
      "height": 905207
    },
    {
      "txid": "7aa9b9970a64165db172dd9323f737da3df4198421669cf80fd03b158a930b26",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 3.82140592,
      "height": 904497
    },
    {
      "txid": "d21e070af5873c7a53f8b7c256761f98ac1f2ee3a2b41a661f0a45354ec29b27",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.02964490,
      "height": 904197
    },
    {
      "txid": "5ebb7e5ae06fc9021c6594f431bf176f5aa6fc899ef3e4aa3f8ea7642071b32c",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.01840367,
      "height": 905032
    },
    {
      "txid": "21d4ff70ebb204ccf3f446965e0bb85497b2ea9204d83e975dfe7af28dfbd12d",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00000948,
      "height": 904518
    },
    {
      "txid": "e29b4113f84159680ef6bbe82f9130ac30256b210c50df60689fe8c2cabf072e",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00002406,
      "height": 905558
    },
    {
      "txid": "bb87d8378b9330ad8c871e44ac7002bfcf6920ff84484952f20c5b125420b131",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00001000,
      "height": 904775
    },
    {
      "txid": "dc1b4792b979b01b3e95161184a09fff84833f39b3ab626767b3c96d71edb533",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.09888828,
      "height": 905081
    },
    {
      "txid": "a7c33cdc276cc8bbb58226bfe5dc5e239fe49e07469a683a976a27a0fe8ae433",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00040831,
      "height": 904870
    },
    {
      "txid": "a70f4cb320ef64743fb0cc3533cea75101f98b2d5ea782348b34c74ef5670c38",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00000780,
      "height": 905202
    },
    {
      "txid": "1af9199e9169932a4ea480e9a0b91a176e2f80d64425cc7b0d48a086ba28c23d",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00137815,
      "height": 904558
    },
    {
      "txid": "69cf7f9031d03edac31cb0c8bd78ed87dc308522b71480344c37b62d3fbb6e3e",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00088510,
      "height": 904131
    },
    {
      "txid": "16e8043850ac28719531b0f6e401633a0354461991d3dd259a3dc4b8a0edca3e",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.89566335,
      "height": 905641
    },
    {
      "txid": "caed1b0db19031996358b352b630a01076c0c28e08e131efa36313810826d340",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00089000,
      "height": 904146
    },
    {
      "txid": "3143b6e9e958fcd85f7de16d036dcadae22007a1e8828e609405379c90614947",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.01481902,
      "height": 905594
    },
    {
      "txid": "e6c687666b3cc0a19d22777f91770ff4141231707916e8baf0dd384afa691948",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00001875,
      "height": 905066
    },
    {
      "txid": "3a63f0974c8d39df4061efd0d43db0e470092e8fe39ea6e0dc8bec9561078e48",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00195000,
      "height": 903776
    },
    {
      "txid": "2eabc349ebb6bd8034d3cac949db37ef6816d19ff49329518b094f5071acb64a",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00005288,
      "height": 905415
    },
    {
      "txid": "136826625fd60ca433ae504ea50c70f17dd40d5f0682bf6fdfe6749077beb24e",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.01113138,
      "height": 904672
    },
    {
      "txid": "ab49044d92450b7e27e45d6037b173df44736ecb98ee351814b71abc0ca9c34e",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00000954,
      "height": 904882
    },
    {
      "txid": "d7edb0fd708ac6a066a323a6a7d5fd42cddaf760a351f2ef8bf84a3acb41064f",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.24384767,
      "height": 904516
    },
    {
      "txid": "c40cb287d597105717e0489797b2afb4639783675d530f32f951c4c5802d4c50",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00547007,
      "height": 903598
    },
    {
      "txid": "6f8cf71035a0b60c4e68ed0b9be1d0cbbcd66a3a87bfc2a2cc6e356e7768e651",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00112548,
      "height": 903654
    },
    {
      "txid": "36a685708cdd57004b72935be746aa937ad753f46d5ef568779fff24fb72ef53",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00010200,
      "height": 898168
    },
    {
      "txid": "e857e2355c498bbb6045228dd98d23cf61770b30f9eae9b468ba20f29e9da455",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00973239,
      "height": 905045
    },
    {
      "txid": "8a309ec550ec2eb2ef79caceb602cd5a217dcae333ff7fe67c938b84ece4785d",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00000744,
      "height": 903945
    },
    {
      "txid": "ec1668bfd5e0436c00cca8d332e4a14fd2d1eb4bcf6b9a1ba1f5f0b04b41b25d",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.02760234,
      "height": 905350
    },
    {
      "txid": "46f2daf45b03c8d2247b06655e29efe320a2b1c083faed94e3f59f5682685e5f",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00033092,
      "height": 904940
    },
    {
      "txid": "4bc47a982a3176069e8f4bc4e2d0110d0ff3ecd1d33aa81b4bfdfade63b28761",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00000918,
      "height": 903868
    },
    {
      "txid": "bbf3a0e12fb46b7192c882fa03e2d048eb127ce4a7174ae5ab6ab0becc0a1664",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00090000,
      "height": 902646
    },
    {
      "txid": "f8fba67eb70df4c88ad1d903c50c2f541664e437350f057bea31b957e98b8064",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00050777,
      "height": 905253
    },
    {
      "txid": "98e874b8dc093aa92690944a8f7c3711e81d3ba5f5b56fc3075a2d5b4c423465",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 9.99989400,
      "height": 903827
    },
    {
      "txid": "380ecdca6243896daffa6e7152dd1121ceec0a0a1fc3cb4e76d10f9fabe61b68",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00529845,
      "height": 904307
    },
    {
      "txid": "0c1ba242f76569d9cef5d21bf29f83c635a2efc1d2646526bfc77439b21c4569",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00076000,
      "height": 904743
    },
    {
      "txid": "fe4a4319f226f912018f97cba78618f6591be9f091fce2d376aef942e1a18a6a",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.01157242,
      "height": 905217
    },
    {
      "txid": "98b9fb4c61c61f9213d4545e9d80351a91e4c79f371fe9afa9ed5f42aa4db470",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00190000,
      "height": 903567
    },
    {
      "txid": "c4d70932d7873d434de92b160a745e349eeb263ffae011fde212d890cda12a73",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00461000,
      "height": 905213
    },
    {
      "txid": "7ac930b1624e207abf3bd70d9de221a6156e51c9956b29a339210baed2a86477",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00001092,
      "height": 903680
    },
    {
      "txid": "7c2a3789b7f2a0f788506a7968f511b76024cde3d0af28c9932d3a923718577b",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00030390,
      "height": 904027
    },
    {
      "txid": "ab43495626f39186237b055601b6bb25b8d047151bb7232678b71c1e84f5a27e",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00322118,
      "height": 904496
    },
    {
      "txid": "92fc7ebffbd6850a10dd4859a9fe7ac85e7e8ed8a60e203c64a5650deed58c80",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00051000,
      "height": 903684
    },
    {
      "txid": "83b0cc60eb0c95829abd9ad6d71c3daae0b805618df3e946ab1479dafcbbc280",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 19.99952184,
      "height": 904626
    },
    {
      "txid": "b378d804724799805f42dc6c319c1af58aa8e1a6758b68e43d2583e13c53a284",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00028163,
      "height": 905374
    },
    {
      "txid": "87f78d536e78c35e1c94f5e08f39fe000ea452a909acffd8db4b85ea4085f686",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00000579,
      "height": 903945
    },
    {
      "txid": "3e5868e713ce0655eb022540d82355248b2cf13dee85161b2c7bdb3087104388",
      "vout": 2,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.26821487,
      "height": 904443
    },
    {
      "txid": "02f76dc542648b798b303226562ae0909cd4b3ffd896e7eadd22336f86b40e89",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.75018870,
      "height": 904870
    },
    {
      "txid": "de88b0ec3084571c3845d94701afafceb3cabec0c76b7839431a2382c8d6ae8a",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00000918,
      "height": 905476
    },
    {
      "txid": "30dfe32376dbccb41dd0115a5747978c0ab154b8d906ba6fa31d51ed987d408d",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00406000,
      "height": 903565
    },
    {
      "txid": "50dfb17c1299ead183fdb49aaa5417b6a92e55835f76a571e78f11c1d95b0d8e",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00059799,
      "height": 904142
    },
    {
      "txid": "51320ba039bb17cf227a0fe156de8724d222af894c4172638eb0da472f667892",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00008238,
      "height": 903567
    },
    {
      "txid": "e7e5bda2ae3cf9cea79e5579732e51a5186970bf5b8ea1a711779e7bbecffa92",
      "vout": 3,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00492680,
      "height": 904500
    },
    {
      "txid": "0810ab8d186215de8f663d4de2fb00fe666d5dc213455adb8b4020d9c2c38396",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00008863,
      "height": 905327
    },
    {
      "txid": "97af3a2de8c6f5d463b09d954b3a72ca848cc72fa591a8ab2cc6bb8a3d98cb99",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00002699,
      "height": 905415
    },
    {
      "txid": "739596e2b468ea45b27f1d5f40b69fab1f1e51ed4bf254047f7cbd85b0654b9d",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00016695,
      "height": 904522
    },
    {
      "txid": "de0613fe79eb8089167c90eaf542b01c6ead67f7081d6710e9d4decbd03862a5",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00001706,
      "height": 905451
    },
    {
      "txid": "5ece66de83110d635e4b5b2fc7a11cdbc4e6303674149ee71046289953dd6ca6",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00090000,
      "height": 904483
    },
    {
      "txid": "791f179c7ae0eb99cbcf1e7877166c5f3488ba9f5de0f1d12b0bfa5096ab94ad",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 5.94216557,
      "height": 904957
    },
    {
      "txid": "567a5d3328fd104900a552e81095be3917e492427f0060e0d512f8d7fb330eaf",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00992000,
      "height": 904601
    },
    {
      "txid": "5b92f6021e8029d507b394472225125e087f7eaad2ab3586e4af7785c7c0e9af",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00041910,
      "height": 899689
    },
    {
      "txid": "919d5e9c6dbcfbfed5a3035c488665424c9b2b8b4d69d1b9f98c90a07c8b35b0",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00478350,
      "height": 899677
    },
    {
      "txid": "ba65a72e1a42508456f7dc573078ca2170b7a592bf0c1e1380ded129632709b1",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00235100,
      "height": 905448
    },
    {
      "txid": "a191ffa5ecd88b721932e7f11e7f02322c929519d61357c08d780edc8c33fbb1",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 15.73915865,
      "height": 904854
    },
    {
      "txid": "d3b2b0c79b5511480920c445d6a6812c65ed9a507202d18f1cc244c90941e2b3",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00020858,
      "height": 904524
    },
    {
      "txid": "6a98e963205baca92c9146fee3b1ab6c8e7e1c8c01c1610e19c269346be19db5",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00386202,
      "height": 905415
    },
    {
      "txid": "f747df9cd0efa4990ac8588fdaebe2535b6a78e1fc6102cd23bfef49f9d2c0b5",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00427075,
      "height": 905265
    },
    {
      "txid": "5c1e2c73de7fd428134cd04b9a5c09bfe6672cafd967d087bdc9114183d8eeb6",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 2.99961791,
      "height": 904317
    },
    {
      "txid": "7479e38f7626d89160547a9180890112460486d07785dad6f117b7cc835466ba",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00077943,
      "height": 904142
    },
    {
      "txid": "4028bcc7c49790c96121e933bca5e93f04ccf9e61eed52394f6cede81eb12bbb",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.02053229,
      "height": 903567
    },
    {
      "txid": "4cda1d173b931fb4709758713641057983217d4b38454e09e88169d64bfba1bb",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00000758,
      "height": 905244
    },
    {
      "txid": "eb08d107666e8b9cc2619159f0d1427a47309ba877229a5e94a2df4f8c41cdbf",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00001275,
      "height": 903945
    },
    {
      "txid": "c2e7be6f3f37f188cc46e522b8eaf3cb4a3145cd7ef54515d5cdaeee13511cc7",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.29242708,
      "height": 905397
    },
    {
      "txid": "1d97a3c06a8ea9e85ed3cacaca925616d2043f5991f04d6430342bfe0e2a3fc8",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.03554840,
      "height": 903418
    },
    {
      "txid": "8a979811c28cf6542cd97358a5c50e425e5e7589053dcc31caa8784423faaecb",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00180963,
      "height": 904318
    },
    {
      "txid": "56a66e66ec565225d313597c22178f5d4b106a46fc74a96202ecf507463906cc",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 9.99970001,
      "height": 901969
    },
    {
      "txid": "2a5500afd3e5d2691492587b21631b0677e25c958ac8fc4dd161549208cdb8cd",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00087096,
      "height": 904937
    },
    {
      "txid": "49deec5d030b50b5c6540e444c66aebeada64230e241009b4be2886b7cb072d0",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00950000,
      "height": 903391
    },
    {
      "txid": "2111e48aeac5f8a2f3451967b1f40252ac1c3049697049fdc168f1eebae724d1",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00971486,
      "height": 904340
    },
    {
      "txid": "8eda0abb2f3dc1b940cb56e1a8efe0a3aa7353a5dc689776f7d8c542f0bb59d1",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00269469,
      "height": 904612
    },
    {
      "txid": "893039aa55b6be069fc8c7c368f8abcc5d3d335c8e31fa5ad823f1212eea2cd5",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00032785,
      "height": 904324
    },
    {
      "txid": "747bc47123338a482f219dff5755e8b6c4bb2d6c37a7a14f91b57725163165d5",
      "vout": 2,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.02897600,
      "height": 904323
    },
    {
      "txid": "3ffcc9130717c25738914e0504602805cc967436db2a95a0781edbd80dc303dc",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00313730,
      "height": 904489
    },
    {
      "txid": "71d4698c78fcd18b4103a83cdd2d454bd6919f27507bfbc3b051e78b008a18dc",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00682110,
      "height": 905386
    },
    {
      "txid": "9e29c613281603cca16b1be5a8bcca2c4a3bc38bf79450b0ebdb8caf134823e1",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.10815552,
      "height": 904343
    },
    {
      "txid": "81a71fc01d9d2045486a441118e120754289d5376c7b3e11185d8060059609e6",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00001814,
      "height": 905449
    },
    {
      "txid": "8fa99adad904472157718aa23a612a8c63f372ab0516a10d486b193fb607b9e6",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00084000,
      "height": 904856
    },
    {
      "txid": "a6e6257e99fb8690a5f0adf17aa9760d726c3a1dd93cbb8778e8b0e14da08bec",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.02213641,
      "height": 900549
    },
    {
      "txid": "7a1c99da0ce75262ec59cffbb0963e885145f0fabc157f93bab1e8f4ac43ecf3",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00031891,
      "height": 904494
    },
    {
      "txid": "d4f1df3e213de0bb2bcb4748fef00d103d55d44af9ffecfc298e5f2aa2a383f5",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00856634,
      "height": 904322
    },
    {
      "txid": "30335c714ee8293ddbd5f97ff09013ba9337ee2d7415e81a7c5cd2a8c2fcdbf6",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 1.79550000,
      "height": 905081
    },
    {
      "txid": "a8cb8857cf6ddde4e19d3b11890b9bc72659aaf9b03cd38debf06257192b77f7",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00008575,
      "height": 904148
    },
    {
      "txid": "74e5f9b6b8378b0f6a29c19570cb3211f96c581f3684c6bbb8da74d29669b1fa",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00000918,
      "height": 903570
    },
    {
      "txid": "175695532095e6701f469fa03bbef59fbef7687ea3de5b932bc0d94c58a501fb",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.01669400,
      "height": 904911
    },
    {
      "txid": "0cde9db130ba81ead97233ff935c3e0bf5d55182448efb0ba4ceb425dbeb5ffc",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00536462,
      "height": 903565
    },
    {
      "txid": "1238d0289d49f2d95bd247aeaaf2aeedc5a3222e964441c9b40b0df93982a1fd",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00774225,
      "height": 905081
    },
    {
      "txid": "5fd64c910a3e8f46ba240b3ac03c19d96a869f7f54197169045015b36c230dff",
      "vout": 1,
      "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
      "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
      "amount": 0.00014403,
      "height": 904167
    }
  ]"#; // Add the rest of your JSON data here

        // Parse the JSON into a Vec<Utxo>
        let utxos: Vec<bitcoincore_rpc::json::Utxo> = serde_json::from_str(json_data)?;

        // Print the results
        println!("Parsed {} UTXOs", utxos.len());

        Ok(utxos)
    }

    /// Add missing change outputs to the UTXO set.
    ///
    /// This is a temporary fix to ensure change outputs are added back to the UTXO set.
    ///   Returns:
    /// - `Ok(())` if successful.
    /// - `Err(SyncError)` if there was an error.
    pub fn add_missing_change_outputs(&mut self) -> Result<(), SyncError> {
        info!("PegoutScheduler::add_missing_change_outputs: Adding missing change outputs to the UTXO set.");

        // Get the UTXO set from the db
        let utxos = self.db.get_all_utxos()?;
        if utxos.is_empty() {
            info!("PegoutScheduler::add_missing_change_outputs: No UTXOs found in the database.");
            return Ok(());
        }
        info!(
            "PegoutScheduler::add_missing_change_outputs: Found {} UTXOs in the database.",
            utxos.len()
        );
        // This is safe because of the empty check above
        let utxo_version = utxos[0].version;

        // Get the unspent utxos for the change address
        let change_utxos = self.change_utxos().expect("change_utxos should parse correctly");

        // Add back missing change outputs to the UTXO set
        let mut missing_utxos = Vec::new();
        for unspent in change_utxos {
            info!(
                "PegoutScheduler::add_missing_change_outputs: Found unspent change output: {:?}",
                unspent
            );

            // Create a UTXO for the change output
            let utxo = Utxo {
                outpoint: OutPoint { txid: unspent.txid, vout: unspent.vout },
                output: TxOut { value: unspent.amount, script_pubkey: unspent.script_pub_key },
                eth_address: None, // Assuming this is None for change outputs
                version: utxo_version,
            };

            // Check if this UTXO already exists in the database by outpoint
            let outpoints = utxos.iter().map(|u| u.outpoint).collect::<HashSet<_>>();
            if !outpoints.contains(&utxo.outpoint) {
                info!(
                    "PegoutScheduler::add_missing_change_outputs: Adding missing change UTXO: {:?}",
                    utxo
                );
                missing_utxos.push(utxo);
            } else {
                // Sanity check that the utxo we create does match the utxo from the db.
                // This ensures that we're creating the correct change outputs.
                let utxo_is_exact_match = utxos.contains(&utxo);
                if !utxo_is_exact_match {
                    let db_utxo = utxos
                        .iter()
                        .find(|u| u.outpoint == utxo.outpoint)
                        .expect("utxo should exist in db");

                    error!(
                        "PegoutScheduler::add_missing_change_outputs: UTXO from db does not match the one we created: {:?} != {:?}",
                        db_utxo, utxo
                    );

                    return Err(SyncError::IncorrectChangeOutputCreated);
                }
            }
        }
        let missing_utxo_refs: Vec<&Utxo> = missing_utxos.iter().collect();
        info!(
            "PegoutScheduler::add_missing_change_outputs: Storing {} missing change outputs.",
            missing_utxo_refs.len()
        );
        self.db.store_utxos(&missing_utxo_refs)?;

        Ok(())
    }

    /// Sync with new blocks and stop when the [checkpoint] block gets finalized.
    ///
    /// We take the database closure to reduce coupling with database module.
    pub fn sync_until(
        &mut self,
        bitcoind: &impl RpcApi,
        checkpoint: BlockHash,
        telemetry: Option<Arc<Telemetry>>,
    ) -> Result<(), SyncError> {
        let cp_result = bitcoind.get_block_header_info(&checkpoint).map_err(SyncError::Rpc)?;

        info!(
            "PegoutScheduler::sync_until: Starting sync: last_finalized={}:{}, target_cp={}:{}",
            print_safe!(bitcoind.get_block_header_info(&self.last_finalized).map(|r| r.height)),
            self.last_finalized,
            cp_result.height,
            checkpoint,
        );

        // If we suspect the node is still syncing, it might have restarted and
        // some of the blocks we already saw might not be in the node's chain.
        // To avoid errors related to this, we'll just ask called to wait.
        if is_syncing(bitcoind)? {
            update_telemetry_error!(telemetry, SyncError::NodeNotSynced);
            return Err(SyncError::NodeNotSynced);
        }

        // Add back missing change outputs to the utxo set.
        // TODO(Scott): This is a temporary fix to ensure change outputs are added back
        // to the UTXO set. We will remove this logic in another release.
        // Check if we have any missing change outputs
        if self.scan_for_change_outputs {
            // Setting to false immediately to avoid infinite retrying
            self.scan_for_change_outputs = false;
            self.add_missing_change_outputs()?;
        }

        // First find the latest block that we have that is still in the blockchain.
        // Make sure that the chain didn't change while we're doing this, so
        // that we know for sure the tip we start working with is the tip of
        // a chain we are actually on.
        let (last, tip) = loop {
            let tip = bitcoind.get_block_header_info(&bitcoind.get_best_block_hash()?)?;
            let last = loop {
                let last = self.last_blocks.back().expect("never empty");
                let in_chain = bitcoind.get_block_header_info(&last.hash)?;
                if in_chain.confirmations > 0 {
                    // Ok, this block is in the chain, we can sync from here.
                    break in_chain;
                } else {
                    if self.last_blocks.len() == 1 {
                        // We rolled back all the blocks we had, so a reorg longer than
                        // our conf_window has taken place. We can't do anything at this point.
                        update_telemetry_error!(telemetry, SyncError::DeepReorg);
                        return Err(SyncError::DeepReorg);
                    }
                    // Our tip got reorged out, eliminate it.
                    self.rollback_tip();
                }
            };

            let new_tip = bitcoind.get_block_header_info(&bitcoind.get_best_block_hash()?)?;
            if tip.hash == new_tip.hash {
                break (last, tip);
            }
            info!("PegoutScheduler::sync_until: Tip changed during reorg check ({} -> {}), retrying...", tip.hash, new_tip.hash);
        };
        if last.height == tip.height {
            assert_eq!(tip.hash, last.hash, "last={:?}, tip={:?}", last, tip);
            return Ok(());
        }

        // Because we need to ensure we are actually syncing a valid chain,
        // we can't just query blocks by height (reorgs might occur along the way).
        // Instead, we first go from the tip and keep a list of hashes to sync,
        // then sync these in the right order.
        debug!("Syncing hashes from {}:{} to {}:{}", last.height, last.hash, tip.height, tip.hash);
        let mut to_sync = Vec::with_capacity(tip.height.saturating_sub(last.height));
        to_sync.push(tip.hash);
        let mut cursor = tip.clone();
        loop {
            let prevhash = cursor.previous_block_hash.expect("can't reach genesis");
            trace!("Getting prev block of {}:{}: {}", cursor.height, cursor.hash, prevhash);
            cursor = bitcoind.get_block_header_info(&prevhash)?;
            if cursor.height == last.height {
                assert_eq!(cursor.hash, last.hash, "last={:?}, tip={:?}", last, tip);
                break;
            }
            to_sync.push(cursor.hash);
        }

        // Then we actually sync all blocks.
        for hash in to_sync.into_iter().rev() {
            if self.last_finalized == checkpoint {
                info!("PegoutScheduler::sync_until: Checkpoint {} reached before processing block {}.", checkpoint, hash);
                break;
            }
            let height = bitcoind.get_block_header_info(&hash)?.height;
            info!("PegoutScheduler::sync_until: Processing block {}:{}", height, hash);
            let block = bitcoind.get_block(&hash)?;
            self.add_block(&block, height);

            if self.last_blocks.len() > self.conf_window as usize {
                let deeply_confirmed_block = self.last_blocks.pop_front().unwrap();
                info!(
                    "PegoutScheduler::sync_until: Block {} is now deeply confirmed ({} blocks deep). Finalizing...",
                    deeply_confirmed_block.hash,
                    self.conf_window
                );
                // Log result of finalize_block
                match self.finalize_block(&deeply_confirmed_block) {
                    Ok(_) => info!(
                        "PegoutScheduler::sync_until: Successfully finalized block {}",
                        deeply_confirmed_block.hash
                    ),
                    Err(e) => {
                        error!(
                            "PegoutScheduler::sync_until: Error finalizing block {}: {}. Propagating error.",
                            deeply_confirmed_block.hash,
                            e
                        );
                        // Propagate ALL database errors immediately
                        // Removed the specific check for Storage variant as it doesn't exist
                        // and we want to propagate any DB error from finalize_block.
                        return Err(SyncError::Db(e));
                    }
                }
            }
        }

        // handle txs that are still in the mempool, have been dropped or there was a reorg
        // this must be done after `finalize_block` which updates the db and pegout scheduler state
        info!("PegoutScheduler::sync_until: Finished block processing loop. Tracking mempool...");
        match self.track_mempool(bitcoind, cp_result) {
            Ok(_) => info!("PegoutScheduler::sync_until: Mempool tracking successful."),
            Err(e) => {
                error!("PegoutScheduler::sync_until: Error during mempool tracking: {}. Propagating error.", e);
                // Decide if mempool tracking error should halt the sync
                return Err(e);
            }
        }

        if self.last_finalized == checkpoint {
            info!("Checkpoint reached: {}", checkpoint);
            Ok(())
        } else {
            let last_info = bitcoind.get_block_header_info(&self.last_finalized);
            let cp_info = bitcoind.get_block_header_info(&checkpoint);
            if let (Ok(last), Ok(cp)) = (last_info, cp_info) {
                debug!(
                    "Checkpoint not reached: last={:?}, checkpoint={:?}, tip={:?}",
                    last, cp, tip
                );
            }
            update_telemetry_error!(telemetry, SyncError::CheckPointNotReached);
            Err(SyncError::CheckPointNotReached)
        }
    }
}

fn is_syncing(bitcoind: &impl RpcApi) -> Result<bool, bitcoincore_rpc::Error> {
    // NB do a raw call with just the initialblockdownload field because this RPC
    // response is quite unstable between releases
    #[derive(Deserialize)]
    struct Res {
        initialblockdownload: bool,
    }
    if bitcoind.call::<Res>("getblockchaininfo", &[])?.initialblockdownload {
        return Ok(true);
    }

    let tip = bitcoind.get_block_header_info(&bitcoind.get_best_block_hash()?)?;
    let elapsed = SystemTime::now().duration_since(tip.block_time()).unwrap_or_default();
    if elapsed > Duration::from_secs(60 * 60) {
        // The tip is over an hour old, node is probably still syncing.
        return Ok(true);
    }

    Ok(false)
}

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("target sync checkpoint not reached yet")]
    CheckPointNotReached,
    #[error("the bitcoind isn't synced yet")]
    NodeNotSynced,
    #[error("bitcoind RPC error: {0}")]
    Rpc(#[from] bitcoincore_rpc::Error),
    #[error("deep reorg wiped out entire index")]
    DeepReorg,
    #[error(transparent)]
    Block(BlockError),
    #[error("database error: {0}")]
    Db(#[from] database::Error),
    #[error("tracked tx not included in a block: {0}")]
    TrackedTxNotInBlock(Txid),
    #[error("deeply confirmed tx not finalized: {0}")]
    DeeplyConfirmedTxNotFinalized(Txid),
    #[error("incorrect change output created")]
    IncorrectChangeOutputCreated,
}

#[derive(Debug, Error)]
pub enum BlockError {
    #[error("failed to connect block to the index")]
    CantConnectBlock,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bitcoin::{
        absolute::LockTime,
        blockdata::{
            script::ScriptBuf,
            transaction::{OutPoint, Sequence, TxOut},
        },
        hashes::Hash,
        transaction::Version,
        TxIn,
    };
    use frost_secp256k1_tr as frost;
    use once_cell::sync::Lazy;

    use crate::{
        frost_id,
        test_utils::{
            create_block, create_random_pegout_id, create_tx, pegout_requests_from_tx,
            random_p2wpkh_script, setup_db, trusted_dealer_setup, MockBitcoind,
        },
    };

    use super::*;

    const MIN_SIGNERS: u16 = 2;
    const MAX_SIGNERS: u16 = 3;

    // A test transaction when you need a deterministic txid which is:
    // ("855b53d27666779a179ec93d88dbe28f456040155c4b712a1261ad211f4ba6f2")
    // This is currently used to test
    // `track_mempool_should_untrack_and_add_back_pegout_when_not_in_mempool()`
    pub static TEST_TRANSACTION_1: Lazy<Transaction> = Lazy::new(|| Transaction {
        version: Version(2),
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([123u8; 32]), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Default::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(1000),
            script_pubkey: ScriptBuf::with_capacity(0),
        }],
    });

    // A test transaction when you need a deterministic txid which is:
    // ("26bbaab2e585d465cceecc2acc7b398069aa85fc4dd1f52e39666a65e54a4569")
    // This is currently used to test
    // `track_mempool_should_not_add_back_pegout_when_still_in_mempool()`
    pub static TEST_TRANSACTION_2: Lazy<Transaction> = Lazy::new(|| Transaction {
        version: Version(2),
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([45u8; 32]), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Default::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(1000),
            script_pubkey: ScriptBuf::with_capacity(0),
        }],
    });

    #[test]
    fn tracked_tx_utils() {
        let tx = create_tx(0, 0, None);
        let tx = Tx {
            txid: tx.compute_txid(),
            tx,
            pegout_idxs: vec![],
            change_idxs: vec![],
            pegout_requests: vec![],
            created: SystemTime::now(),
        };

        assert_eq!(tx.inputs().count(), 0);
        assert_eq!(tx.pegouts().count(), 0);
        assert_eq!(tx.change().count(), 0);

        // 5 inputs, 2 outputs
        let dummy_tx = create_tx(5, 2, None);
        assert_eq!(dummy_tx.input.len(), 5);
        assert_eq!(dummy_tx.output.len(), 2);

        let tx2 = Tx {
            txid: dummy_tx.compute_txid(),
            tx: dummy_tx.clone(),
            pegout_idxs: vec![0],
            change_idxs: vec![1],
            created: SystemTime::now(),
            pegout_requests: vec![],
        };

        assert_eq!(tx2.inputs().count(), 5);
        assert_eq!(tx2.pegouts().count(), 1);
        assert_eq!(tx2.change().count(), 1);

        assert_eq!(dummy_tx.output[0], tx2.pegouts().next().unwrap().1.clone());
        assert_eq!(dummy_tx.output[1], tx2.change().next().unwrap().1.clone());
    }

    #[test]
    fn test_track_tx() {
        let db = setup_db().0;
        let (shares, pk_package) = trusted_dealer_setup(MIN_SIGNERS, MAX_SIGNERS);
        let key_package = frost::keys::KeyPackage::try_from(shares[&frost_id!(1u16)].clone())
            .expect("valid key package");

        db.set_pubkey_package(pk_package).expect("set public key package");
        db.set_key_package(key_package).expect("set key package");

        let agg_pk =
            db.get_public_key_package().unwrap().unwrap().verifying_key().to_secp_pk().unwrap();
        let change_spk = generate_taproot_change_scriptpubkey(&agg_pk);
        let change_output = TxOut { value: Amount::from_sat(1000), script_pubkey: change_spk };
        let tx = create_tx(3, 3, Some(change_output.clone()));
        let pegout_idxs = vec![0, 1, 2];
        let change_idxs = vec![3];

        let mut pegout_scheduler =
            PegoutScheduler::new(101, vec![], bitcoin::BlockHash::all_zeros(), db);

        let mut pegouts = vec![];
        for i in pegout_idxs.iter() {
            let pegout_req = PegoutRequest {
                spk: tx.output[*i].script_pubkey.clone(),
                value: tx.output[*i].value,
                id: create_random_pegout_id(),
                botanix_height: 0,
            };
            pegouts.push(pegout_req);
        }
        assert_eq!(pegouts.len(), 3);
        pegout_scheduler.add_tx(tx.clone(), &pegouts, SystemTime::now());

        let pending_txs = pegout_scheduler.txs.clone();
        assert_eq!(pending_txs.len(), 1);
        let (pending_txid, pending_tx) = pending_txs.into_iter().next().unwrap();
        assert_eq!(pending_txid, tx.compute_txid());
        assert_eq!(pending_tx.pegout_idxs, pegout_idxs);
        assert_eq!(pending_tx.change_idxs, change_idxs);

        // Check the mapping is correct
        let txs_by_pegout = pegout_scheduler.txs_by_pegout.clone();
        assert_eq!(txs_by_pegout.len(), 3);
        for pegout in pegouts.iter() {
            assert_eq!(txs_by_pegout.get(&pegout.txout()).unwrap(), &vec![tx.compute_txid()]);
        }

        let tx_by_input = pegout_scheduler.txs_by_input.clone();
        assert_eq!(tx_by_input.len(), 3);
        for input in tx.input.iter() {
            assert_eq!(tx_by_input.get(&input.previous_output).unwrap(), &vec![tx.compute_txid()]);
        }

        let tracked_inputs = pegout_scheduler.tracked_inputs();
        assert_eq!(tracked_inputs.len(), 3);
        for input in tx.input.iter() {
            assert!(tracked_inputs.contains(&input.previous_output));
        }

        // adding the same tx again should not change anything
        pegout_scheduler.add_tx(tx.clone(), &pegouts, SystemTime::now());
        assert_eq!(pegout_scheduler.txs.len(), 1);
        let pending_txs = pegout_scheduler.txs.clone();
        let (pending_txid, pending_tx) = pending_txs.into_iter().next().unwrap();
        assert_eq!(pending_txid, tx.compute_txid());
        assert_eq!(pending_tx.pegout_idxs, pegout_idxs);
        assert_eq!(pending_tx.change_idxs, change_idxs);
    }

    #[test]
    fn test_add_block() {
        let db = setup_db().0;
        let (shares, pk_package) = trusted_dealer_setup(MIN_SIGNERS, MAX_SIGNERS);
        let key_package = frost::keys::KeyPackage::try_from(shares[&frost_id!(1u16)].clone())
            .expect("valid key package");

        db.set_pubkey_package(pk_package).expect("set public key package");
        db.set_key_package(key_package).expect("set key package");

        let mut pegout_scheduler =
            PegoutScheduler::new(101, vec![], bitcoin::BlockHash::all_zeros(), db);
        let tx1 = create_tx(3, 3, None);
        let tx2 = create_tx(3, 3, None);
        let txs = vec![tx1.clone(), tx2.clone()];
        let pegouts1 = pegout_requests_from_tx(&tx1, &[0, 1]);
        let pegouts2 = pegout_requests_from_tx(&tx2, &[0, 1]);

        pegout_scheduler.add_tx(tx1.clone(), &pegouts1, SystemTime::now());
        pegout_scheduler.add_tx(tx2.clone(), &pegouts2, SystemTime::now());
        assert_eq!(pegout_scheduler.txs.len(), 2);

        let block = create_block(txs, bitcoin::BlockHash::all_zeros());
        pegout_scheduler.add_block(&block, 1);

        let last_blocks = pegout_scheduler.last_blocks;
        assert_eq!(last_blocks.len(), 2);
        let last_block = last_blocks.back().unwrap();
        assert_eq!(last_block.relevant_txs.len(), 2);
        assert_eq!(last_block.relevant_inputs.len(), 0);
        assert_eq!(last_block.hash, block.block_hash());

        let txs = last_block.relevant_txs.clone();
        assert!(txs.contains(&tx1.compute_txid()));
        assert!(txs.contains(&tx2.compute_txid()));
    }

    #[test]
    fn test_finalize_block() {
        let db = setup_db().0;
        let (shares, pk_package) = trusted_dealer_setup(MIN_SIGNERS, MAX_SIGNERS);
        let key_package = frost::keys::KeyPackage::try_from(shares[&frost_id!(1u16)].clone())
            .expect("valid key package");

        db.set_pubkey_package(pk_package).expect("set public key package");
        db.set_key_package(key_package).expect("set key package");

        let agg_pk =
            db.get_public_key_package().unwrap().unwrap().verifying_key().to_secp_pk().unwrap();
        let change_spk = generate_taproot_change_scriptpubkey(&agg_pk);
        let change_output = TxOut { value: Amount::from_sat(1000), script_pubkey: change_spk };

        let mut pegout_scheduler =
            PegoutScheduler::new(101, vec![], bitcoin::BlockHash::all_zeros(), db.clone());
        let tx1 = create_tx(3, 3, Some(change_output.clone()));
        let tx2 = create_tx(3, 3, Some(change_output));
        let txs = vec![tx1.clone(), tx2.clone()];
        let pegouts1 = pegout_requests_from_tx(&tx1, &[0, 1, 2]);
        let pegouts2 = pegout_requests_from_tx(&tx2, &[0, 1, 2]);

        pegout_scheduler.add_tx(tx1.clone(), &pegouts1, SystemTime::now());
        pegout_scheduler.add_tx(tx2.clone(), &pegouts2, SystemTime::now());

        let block = create_block(txs, bitcoin::BlockHash::all_zeros());
        pegout_scheduler.add_block(&block, 1);
        let last_blocks = pegout_scheduler.last_blocks.clone();
        assert_eq!(last_blocks.len(), 2);
        let last_block = last_blocks.back().unwrap();
        pegout_scheduler.finalize_block(last_block).expect("finalize block");

        assert_eq!(pegout_scheduler.last_finalized, block.block_hash());

        let tracked_txs = pegout_scheduler.txs.clone();
        let utxos = db.get_all_utxos().unwrap();
        assert_eq!(tracked_txs.len(), 0);
        assert_eq!(utxos.len(), 2);

        let db_tracked_txs = db.get_tracked_txs().unwrap();
        assert_eq!(db_tracked_txs.len(), 0);

        // Check the correct last finalized block hash is correct
        assert_eq!(pegout_scheduler.last_finalized, block.block_hash());
    }

    #[test]
    fn finalizing_one_change_output() {
        let db = setup_db().0;
        let (shares, pk_package) = trusted_dealer_setup(MIN_SIGNERS, MAX_SIGNERS);
        let key_package = frost::keys::KeyPackage::try_from(shares[&frost_id!(1u16)].clone())
            .expect("valid key package");

        db.set_pubkey_package(pk_package).expect("set public key package");
        db.set_key_package(key_package).expect("set key package");

        let agg_pk =
            db.get_public_key_package().unwrap().unwrap().verifying_key().to_secp_pk().unwrap();
        let change_spk = generate_taproot_change_scriptpubkey(&agg_pk);
        let change_output = TxOut { value: Amount::from_sat(1000), script_pubkey: change_spk };

        let mut pegout_scheduler =
            PegoutScheduler::new(101, vec![], bitcoin::BlockHash::all_zeros(), db.clone());
        let tx = create_tx(3, 1, Some(change_output));
        let pegouts = pegout_requests_from_tx(&tx, &[0]);
        pegout_scheduler.add_tx(tx.clone(), &pegouts, SystemTime::now());

        let (last_tx_txid, last_tx) = pegout_scheduler.txs.clone().into_iter().next().unwrap();
        assert_eq!(last_tx_txid, tx.compute_txid());
        assert_eq!(last_tx.pegout_idxs, vec![0]);
        assert_eq!(last_tx.change_idxs, vec![1]);

        let block = create_block(vec![tx], bitcoin::BlockHash::all_zeros());
        pegout_scheduler.add_block(&block, 1);

        let last_blocks = pegout_scheduler.last_blocks.clone();
        let last_block = last_blocks.back().unwrap();

        pegout_scheduler.finalize_block(last_block).expect("finalize block");

        let utxos = db.get_all_utxos().unwrap();
        // there is now one change so there is one UTXO to add back to UTXO set
        assert_eq!(utxos.len(), 1);
    }

    #[test]
    fn finalizing_incorrect_tracked_output() {
        // here we are tracking tx where all outputs are pegouts but one is mistaken as change
        // The result should be that the incorrect change is NOT added back to UTXO set
        let db = setup_db().0;
        let (shares, pk_package) = trusted_dealer_setup(MIN_SIGNERS, MAX_SIGNERS);
        let key_package = frost::keys::KeyPackage::try_from(shares[&frost_id!(1u16)].clone())
            .expect("valid key package");

        db.set_pubkey_package(pk_package).expect("set public key package");
        db.set_key_package(key_package).expect("set key package");

        let mut pegout_scheduler =
            PegoutScheduler::new(101, vec![], bitcoin::BlockHash::all_zeros(), db.clone());
        let tx = create_tx(3, 2, None);
        // Here we should be tracking indices 0 and 1.
        // But we are tracking 0 as a pegout, therefore output 1 is going to be mistaken as change
        let pegouts = pegout_requests_from_tx(&tx, &[0]);
        pegout_scheduler.add_tx(tx.clone(), &pegouts, SystemTime::now());

        let (last_tx_txid, last_tx) = pegout_scheduler.txs.clone().into_iter().next().unwrap();
        assert_eq!(last_tx_txid, tx.compute_txid());
        assert_eq!(last_tx.pegout_idxs, vec![0]);
        // should be empty since we check against change.spk during add_tx
        assert!(last_tx.change_idxs.is_empty());

        let block = create_block(vec![tx], bitcoin::BlockHash::all_zeros());
        pegout_scheduler.add_block(&block, 1);

        let last_blocks = pegout_scheduler.last_blocks.clone();
        let last_block = last_blocks.back().unwrap();

        pegout_scheduler.finalize_block(last_block).expect("finalize block");

        let utxos = db.get_all_utxos().unwrap();
        // No change so there is no UTXO to add back to UTXO set
        assert_eq!(utxos.len(), 0);
    }

    #[test]
    fn finalizing_incorrect_change_output() {
        // Here we are tracking tx where the incorrect change is registered
        // The result should be that the incorrect change is NOT added back to UTXO set
        let db = setup_db().0;
        let (shares, pk_package) = trusted_dealer_setup(MIN_SIGNERS, MAX_SIGNERS);
        let key_package = frost::keys::KeyPackage::try_from(shares[&frost_id!(1u16)].clone())
            .expect("valid key package");

        db.set_pubkey_package(pk_package).expect("set public key package");
        db.set_key_package(key_package).expect("set key package");
        let mut pegout_scheduler =
            PegoutScheduler::new(101, vec![], bitcoin::BlockHash::all_zeros(), db.clone());

        let incorrect_change_spk = random_p2wpkh_script();
        let tx = create_tx(
            3,
            2,
            Some(TxOut { value: Amount::from_sat(1000), script_pubkey: incorrect_change_spk }),
        );

        let pegouts = pegout_requests_from_tx(&tx, &[0, 1]);
        pegout_scheduler.add_tx(tx.clone(), &pegouts, SystemTime::now());

        let (last_tx_txid, last_tx) = pegout_scheduler.txs.clone().into_iter().next().unwrap();
        assert_eq!(last_tx_txid, tx.compute_txid());
        assert_eq!(last_tx.pegout_idxs, vec![0, 1]);
        // should be empty since we check against change.spk during add_tx
        assert!(last_tx.change_idxs.is_empty());

        let block = create_block(vec![tx], bitcoin::BlockHash::all_zeros());
        pegout_scheduler.add_block(&block, 1);

        let last_blocks = pegout_scheduler.last_blocks.clone();
        let last_block = last_blocks.back().unwrap();

        pegout_scheduler.finalize_block(last_block).expect("finalize block");

        let utxos = db.get_all_utxos().unwrap();
        // No change so there is no UTXO to add back to UTXO set
        assert_eq!(utxos.len(), 0);
    }

    #[test]
    fn start_with_existing_tracked_txs() {
        let db = setup_db().0;
        let tx = create_tx(1, 2, None);
        let pegouts = pegout_requests_from_tx(&tx, &[0]);
        let tracked_tx = Tx {
            txid: tx.compute_txid(),
            tx: tx.clone(),
            pegout_idxs: vec![0],
            change_idxs: vec![1],
            created: SystemTime::now(),
            pegout_requests: pegouts,
        };

        let pegout_scheduler =
            PegoutScheduler::new(1, vec![tracked_tx], bitcoin::BlockHash::all_zeros(), db);
        let (last_tx_txid, last_tx) = pegout_scheduler.txs.clone().into_iter().next().unwrap();
        assert_eq!(last_tx_txid, tx.compute_txid());
        assert_eq!(last_tx.pegout_idxs, vec![0]);
        assert_eq!(last_tx.change_idxs, vec![1]);
    }

    #[test]
    fn test_finalize_many_blocks() {
        let db = setup_db().0;
        let (shares, pk_package) = trusted_dealer_setup(MIN_SIGNERS, MAX_SIGNERS);
        let key_package = frost::keys::KeyPackage::try_from(shares[&frost_id!(1u16)].clone())
            .expect("valid key package");

        db.set_pubkey_package(pk_package).expect("set public key package");
        db.set_key_package(key_package).expect("set key package");
        let agg_pk =
            db.get_public_key_package().unwrap().unwrap().verifying_key().to_secp_pk().unwrap();
        let change_spk = generate_taproot_change_scriptpubkey(&agg_pk);
        let change_output = TxOut { value: Amount::from_sat(1000), script_pubkey: change_spk };

        let mut pegout_scheduler =
            PegoutScheduler::new(1, vec![], bitcoin::BlockHash::all_zeros(), db.clone());
        let mut last_block_hash = bitcoin::BlockHash::all_zeros();

        for _ in 0..100 {
            let tx = create_tx(1, 2, Some(change_output.clone()));
            let pegouts = pegout_requests_from_tx(&tx, &[0]);
            pegout_scheduler.add_tx(tx.clone(), &pegouts, SystemTime::now());
            let block = create_block(vec![tx], last_block_hash);
            pegout_scheduler.add_block(&block, 1);
            let last_blocks = pegout_scheduler.last_blocks.clone();

            let last_block = last_blocks.back().unwrap();
            last_block_hash = last_block.hash;
            pegout_scheduler.finalize_block(last_block).expect("finalize block");
        }
        // 100 change outputs are added back to UTXO set
        let utxos = db.get_all_utxos().unwrap();
        assert_eq!(utxos.len(), 100);
    }

    #[test]
    fn test_un_track_tx() {
        let db = setup_db().0;
        let tx = create_tx(1, 2, None);
        let pegouts = pegout_requests_from_tx(&tx, &[0]);
        let tracked_tx = Tx {
            txid: tx.compute_txid(),
            tx: tx.clone(),
            pegout_idxs: vec![0],
            change_idxs: vec![1],
            created: SystemTime::now(),
            pegout_requests: pegouts,
        };

        let mut pegout_scheduler =
            PegoutScheduler::new(1, vec![tracked_tx], bitcoin::BlockHash::all_zeros(), db);
        assert_eq!(pegout_scheduler.txs.len(), 1);

        pegout_scheduler.un_track_tx(&tx.compute_txid()).expect("untrack tx");
        // Check the mapping is correct
        let txs_by_pegout = pegout_scheduler.txs_by_pegout.clone();
        assert_eq!(txs_by_pegout.len(), 0);

        let tx_by_input = pegout_scheduler.txs_by_input.clone();
        assert_eq!(tx_by_input.len(), 0);

        let tracked_inputs = pegout_scheduler.tracked_inputs();
        assert_eq!(tracked_inputs.len(), 0);

        assert_eq!(pegout_scheduler.txs.len(), 0);
    }

    #[test]
    fn test_roll_back_tip() {
        let db = setup_db().0;
        let (shares, pk_package) = trusted_dealer_setup(MIN_SIGNERS, MAX_SIGNERS);
        let key_package = frost::keys::KeyPackage::try_from(shares[&frost_id!(1u16)].clone())
            .expect("valid key package");
        db.set_pubkey_package(pk_package).expect("set public key package");
        db.set_key_package(key_package).expect("set key package");

        let mut pegout_scheduler =
            PegoutScheduler::new(1, vec![], bitcoin::BlockHash::all_zeros(), db.clone());
        let tx = create_tx(1, 2, None);
        let pegouts = pegout_requests_from_tx(&tx, &[0]);

        let tracked_tx = pegout_scheduler.add_tx(tx.clone(), &pegouts, SystemTime::now());
        // Add to db as well
        db.store_tracked_tx(&tracked_tx).unwrap();
        assert_eq!(pegout_scheduler.txs.len(), 1);

        // Add a block
        let block = BlockInfo {
            hash: bitcoin::BlockHash::all_zeros(),
            relevant_txs: vec![tx.compute_txid()],
            relevant_inputs: vec![],
        };
        pegout_scheduler.last_blocks.push_back(block);
        // Dont need to worry about last finalized
        pegout_scheduler.rollback_tip();
        assert_eq!(pegout_scheduler.txs.len(), 0);

        let pending_pegouts = db.get_pending_pegouts().unwrap();
        assert_eq!(pending_pegouts[0], pegouts[0]);
    }

    #[test]
    fn tracked_pegout_request_ids_should_return_ids() {
        let db = setup_db().0;
        let tx = create_tx(1, 2, None);
        let pegouts = pegout_requests_from_tx(&tx, &[0]);
        let tracked_tx = Tx {
            txid: tx.compute_txid(),
            tx: tx.clone(),
            pegout_idxs: vec![0],
            change_idxs: vec![1],
            created: SystemTime::now(),
            pegout_requests: pegouts.clone(),
        };

        let pegout_scheduler =
            PegoutScheduler::new(1, vec![tracked_tx], bitcoin::BlockHash::all_zeros(), db);
        let pegout_request_ids = pegout_scheduler.tracked_pegout_request_ids();
        assert_eq!(pegout_request_ids.len(), 1);
        assert_eq!(pegout_request_ids[0], pegouts[0].id);
    }

    #[test]
    // mock_bitcoind is set up so the tracked tx is confirmed but not deeply confirmed.
    fn track_mempool_should_not_add_back_pegout_when_not_deeply_confirmed() {
        let db = setup_db().0;
        let (shares, pk_package) = trusted_dealer_setup(MIN_SIGNERS, MAX_SIGNERS);
        let key_package = frost::keys::KeyPackage::try_from(shares[&frost_id!(1u16)].clone())
            .expect("valid key package");
        db.set_pubkey_package(pk_package).expect("set public key package");
        db.set_key_package(key_package).expect("set key package");

        let mut pegout_scheduler =
            PegoutScheduler::new(101, vec![], bitcoin::BlockHash::all_zeros(), db.clone());
        let tx = create_tx(1, 2, None);
        let pegouts = pegout_requests_from_tx(&tx, &[0]);
        pegout_scheduler.add_tx(tx.clone(), &pegouts, SystemTime::now());

        let mock_bitcoind = MockBitcoind::new();
        let mut checkpoint = mock_bitcoind
            .get_block_header_info(&bitcoin::BlockHash::all_zeros())
            .expect("valid checkpoint");
        // increase time for checkpoint block so tracked tx is older
        checkpoint.time += 5;

        let result = pegout_scheduler.track_mempool(&mock_bitcoind, checkpoint);
        assert!(result.is_ok());

        // assert the pegout was added to pending pegouts
        let pending_pegouts = db.get_pending_pegouts().expect("pending pegouts exist");
        assert_eq!(pending_pegouts.len(), 0);
    }

    #[test]
    fn track_mempool_should_not_add_back_pegout_when_still_in_mempool() {
        let db = setup_db().0;
        let (shares, pk_package) = trusted_dealer_setup(MIN_SIGNERS, MAX_SIGNERS);
        let key_package = frost::keys::KeyPackage::try_from(shares[&frost_id!(1u16)].clone())
            .expect("valid key package");
        db.set_pubkey_package(pk_package).expect("set public key package");
        db.set_key_package(key_package).expect("set key package");

        let mut pegout_scheduler =
            PegoutScheduler::new(101, vec![], bitcoin::BlockHash::all_zeros(), db.clone());
        // mock bitcoind will trigger error path for `getmempoolentry` for specific txids
        // so pass true to create_tx() to make it deterministic which is
        // "26bbaab2e585d465cceecc2acc7b398069aa85fc4dd1f52e39666a65e54a4569" for this test
        // this txid will result in the tx not being on chain but in the mempool
        let pegouts = pegout_requests_from_tx(&TEST_TRANSACTION_2, &[0]);
        pegout_scheduler.add_tx(TEST_TRANSACTION_2.clone(), &pegouts, SystemTime::now());

        let mock_bitcoind = MockBitcoind::new();
        let mut checkpoint = mock_bitcoind
            .get_block_header_info(&bitcoin::BlockHash::all_zeros())
            .expect("valid checkpoint");
        // increase time for checkpoint block so tracked tx is older
        checkpoint.time += 5;

        let result = pegout_scheduler.track_mempool(&mock_bitcoind, checkpoint);
        assert!(result.is_ok());

        // assert the pegout was added to pending pegouts
        let pending_pegouts = db.get_pending_pegouts().expect("pending pegouts exist");
        assert_eq!(pending_pegouts.len(), 0);
    }

    #[test]
    fn track_mempool_should_untrack_and_add_back_pegout_when_not_in_mempool() {
        let db = setup_db().0;
        let mut pegout_scheduler =
            PegoutScheduler::new(101, vec![], bitcoin::BlockHash::all_zeros(), db.clone());
        // mock bitcoind will trigger error path for `getmempoolentry` for a specific txid
        // so pass true to create_tx() to make it deterministic which is
        // "855b53d27666779a179ec93d88dbe28f456040155c4b712a1261ad211f4ba6f2" for this test
        // this txid will result in the tx not being on chain nor in the mempool
        let pegouts = pegout_requests_from_tx(&TEST_TRANSACTION_1, &[0]);
        pegout_scheduler.add_tx(TEST_TRANSACTION_1.clone(), &pegouts, SystemTime::now());

        let mock_bitcoind = MockBitcoind::new();
        let mut checkpoint = mock_bitcoind
            .get_block_header_info(&bitcoin::BlockHash::all_zeros())
            .expect("valid checkpoint");
        // increase time for checkpoint block so tracked tx is older
        checkpoint.time += 5;

        // assert pending pegouts is empty
        let pending_pegouts = db.get_pending_pegouts().expect("pending pegouts exist");
        assert!(pending_pegouts.is_empty());

        let result = pegout_scheduler.track_mempool(&mock_bitcoind, checkpoint);
        assert!(result.is_ok());

        // assert the pegout was added to pending pegouts
        let pending_pegouts = db.get_pending_pegouts().expect("pending pegouts exist");
        assert_eq!(pending_pegouts.len(), 1);
        assert_eq!(pending_pegouts[0], pegouts[0]);

        // assert there are no tracked txs
        assert_eq!(pegout_scheduler.txs.len(), 0);
    }

    #[test]
    fn test_add_missing_change_outputs() {
        let db = setup_db().0;
        let mut pegout_scheduler =
            PegoutScheduler::new(101, vec![], bitcoin::BlockHash::all_zeros(), db.clone());

        // Add utxo to db
        let tx = create_tx(2, 1, None);
        let utxo = Utxo::new(
            OutPoint::new(tx.compute_txid(), 0),
            tx.output.get(0).unwrap().clone(),
            None,
            None,
        );
        db.store_utxos(&[&utxo]).unwrap();
        db.flush().unwrap();

        let change = pegout_scheduler.change_utxos().expect("change_utxos should parse correctly");

        pegout_scheduler
            .add_missing_change_outputs()
            .expect("add_missing_change_outputs should succeed");

        let utxos = db.get_all_utxos().unwrap();

        // The db should have all the change outputs plus one added during db setup
        assert_eq!(utxos.len(), change.len() + 1);
    }
}
