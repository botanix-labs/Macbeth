/// This module is responsible for tracking pegout transactions and detecting when they are
/// confirmed or when they need to be retried.
/// Some vocab used in this file
/// - tracked tx: a transaction that is confirmed but not sufficiently deep.
/// - pending pegout: a pegout that is pending to be signed and broadcasted.
/// - confirmed tx: a transaction that has > 1 confirmation.
/// - finalized tx: a transaction that is deeply confirmed.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, SystemTime},
};

use bitcoin::{Amount, Block, BlockHash, OutPoint, ScriptBuf, Transaction, TxOut, Txid};
use bitcoincore_rpc::RpcApi;
use reth_btc_wallet::{
    address::generate_taproot_change_scriptpubkey,
    util::{VerifyingKeyExt, VerifyingKeyExtError},
};
use thiserror::Error;

use crate::{database, pegout_id::PegoutId};

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
    /// sufficientlydeep confirmations on L1. Once they do change outputs may
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
            for pegout in pegouts {
                let pegout_txout = pegout.txout();
                let idx = tx
                    .output
                    .iter()
                    .position(|txout| *txout == pegout_txout)
                    .expect("tx doesn't contain all pegouts");
                ret.push(idx);
            }
            ret
        };
        let change_idxs = {
            let mut ret = Vec::with_capacity(pegout_idxs.len());
            for (i, _) in tx.output.iter().enumerate() {
                if pegout_idxs.contains(&i) {
                    continue;
                }
                ret.push(i);
            }
            ret
        };
        let txid = tx.txid();
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
    fn un_track_tx(&mut self, txid: &Txid) -> Result<(), database::Error> {
        let tx = self.txs.get(txid).expect("relevant tx should exist");
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
        for pegout in tx.pegout_requests.iter() {
            self.db.store_pending_pegout(pegout)?;
        }
        Ok(())
    }

    fn rollback_tip(&mut self) {
        assert!(!self.last_blocks.is_empty());
        let drop = self.last_blocks.pop_back().unwrap();
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
        let change_spk = self.get_change_spk().expect("dkg should have been performed");
        // To make sure we only update the index when the db is also synced,
        // first try store the new finalized UTXOs to the db, then update the index.
        let mut all_inputs = block.relevant_inputs.iter().copied().collect::<HashSet<_>>();
        for txid in &block.relevant_txs {
            let tx = self.txs.get(txid).expect("corrupt db");
            // Add back the change to the utxo set
            let mut change_utxos = vec![];
            for (outpoint, output) in tx.change() {
                if output.script_pubkey != change_spk {
                    warn!("Change output being tracked is not a p2tr: {:?}", output);
                    continue;
                }
                change_utxos.push(database::Utxo {
                    outpoint,
                    output: output.clone(),
                    eth_address: None,
                });
            }
            self.db.store_utxos(change_utxos.iter().collect::<Vec<_>>().as_slice())?;
            self.db.flush()?;
            all_inputs.extend(tx.tx.input.iter().map(|i| i.previous_output));
        }

        // Now that it's all in the db, we can apply changes here.
        for input in all_inputs {
            // Remove the tracked tx from the database as well
            self.db.remove_tracked_tx(&input.txid)?;
            // Remove local copy
            if let Some(tx) = self.txs.remove(&input.txid) {
                info!("Dropping tx that conflicts with finalized tx: {:?}", tx);
            }
            // Those inputs are no longer worth tracking
            // And no longer spendable, we can safely remove them
            self.db.remove_utxo(&input)?;
            self.db.flush()?;
        }
        for txid in &block.relevant_txs {
            self.txs.remove(txid);
        }

        self.last_finalized = block.hash;

        Ok(())
    }

    /// Adds a new block to the chain.
    ///
    /// Updates the [SyncResult] with the data from newly finalized blocks.
    fn add_block(&mut self, block: &Block) {
        let hash: BlockHash = block.block_hash();
        let height = block.bip34_block_height().expect("bip34 is active");
        let last = self.last_blocks.back().expect("always something");
        assert_eq!(block.header.prev_blockhash, last.hash, "adding {}:{}", height, hash);

        let mut relevant_txs = Vec::new();
        let mut relevant_inputs = Vec::new();
        for tx in &block.txdata {
            let txid = tx.txid();
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

    /// Sync with new blocks and stop when the [checkpoint] block gets finalized.
    ///
    /// We take the database closure to reduce coupling with database module.
    pub fn sync_until(
        &mut self,
        bitcoind: &impl RpcApi,
        checkpoint: BlockHash,
    ) -> Result<(), SyncError> {
        info!(
            "Syncing pegout scheduler: last={}:{}, cp={}:{}",
            print_safe!(bitcoind.get_block_header_info(&self.last_finalized).map(|r| r.height)),
            self.last_finalized,
            print_safe!(bitcoind.get_block_header_info(&checkpoint).map(|r| r.height)),
            checkpoint,
        );

        // If we suspect the node is still syncing, it might have restarted and
        // some of the blocks we already saw might not be in the node's chain.
        // To avoid errors related to this, we'll just ask called to wait.
        if is_syncing(bitcoind)? {
            return Err(SyncError::NodeNotSynced);
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
                debug!("Checkpoint reached: {}", checkpoint);
                break;
            }

            let block = bitcoind.get_block(&hash)?;
            self.add_block(&block);

            if self.last_blocks.len() > self.conf_window as usize {
                let deeply_confirmed_block = self.last_blocks.pop_front().unwrap();
                self.finalize_block(&deeply_confirmed_block)?;
            }
        }

        // Here we can additionally check for txs are tracked for a long periods of time
        // And no longer are in the mempool
        // TODO

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
}

#[derive(Debug, Error)]
pub enum BlockError {
    #[error("failed to connect block to the index")]
    CantConnectBlock,
}

#[cfg(test)]
mod tests {
    use bitcoin::hashes::Hash;
    use frost_secp256k1_tr as frost;

    use crate::{
        frost_id,
        test_utils::test_utils::{
            create_block, create_random_pegout_id, create_tx, pegout_requests_from_tx,
            random_p2wpkh_script, setup_db, trusted_dealer_setup,
        },
    };

    use super::*;

    const MIN_SIGNERS: u16 = 2;
    const MAX_SIGNERS: u16 = 3;

    #[test]
    fn tracked_tx_utils() {
        let tx = create_tx(0, 0, None);
        let tx = Tx {
            txid: tx.txid(),
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
            txid: dummy_tx.txid(),
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

        let tx = create_tx(3, 3, None);
        let pegout_idxs = vec![0, 1];
        let change_idxs = vec![2];

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
        assert_eq!(pegouts.len(), 2);
        pegout_scheduler.add_tx(tx.clone(), &pegouts, SystemTime::now());

        let pending_txs = pegout_scheduler.txs.clone();
        assert_eq!(pending_txs.len(), 1);
        let (pending_txid, pending_tx) = pending_txs.into_iter().next().unwrap();
        assert_eq!(pending_txid, tx.txid());
        assert_eq!(pending_tx.pegout_idxs, pegout_idxs);
        assert_eq!(pending_tx.change_idxs, change_idxs);

        // Check the mapping is correct
        let txs_by_pegout = pegout_scheduler.txs_by_pegout.clone();
        assert_eq!(txs_by_pegout.len(), 2);
        for pegout in pegouts.iter() {
            assert_eq!(txs_by_pegout.get(&pegout.txout()).unwrap(), &vec![tx.txid()]);
        }

        let tx_by_input = pegout_scheduler.txs_by_input.clone();
        assert_eq!(tx_by_input.len(), 3);
        for input in tx.input.iter() {
            assert_eq!(tx_by_input.get(&input.previous_output).unwrap(), &vec![tx.txid()]);
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
        assert_eq!(pending_txid, tx.txid());
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
        pegout_scheduler.add_block(&block);

        let last_blocks = pegout_scheduler.last_blocks;
        assert_eq!(last_blocks.len(), 2);
        let last_block = last_blocks.back().unwrap();
        assert_eq!(last_block.relevant_txs.len(), 2);
        assert_eq!(last_block.relevant_inputs.len(), 0);
        assert_eq!(last_block.hash, block.block_hash());

        let txs = last_block.relevant_txs.clone();
        assert!(txs.contains(&tx1.txid()));
        assert!(txs.contains(&tx2.txid()));
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
        pegout_scheduler.add_block(&block);
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
        assert_eq!(last_tx_txid, tx.txid());
        assert_eq!(last_tx.pegout_idxs, vec![0]);
        assert_eq!(last_tx.change_idxs, vec![1]);

        let block = create_block(vec![tx], bitcoin::BlockHash::all_zeros());
        pegout_scheduler.add_block(&block);

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
        assert_eq!(last_tx_txid, tx.txid());
        assert_eq!(last_tx.pegout_idxs, vec![0]);
        assert_eq!(last_tx.change_idxs, vec![1]);

        let block = create_block(vec![tx], bitcoin::BlockHash::all_zeros());
        pegout_scheduler.add_block(&block);

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
        assert_eq!(last_tx_txid, tx.txid());
        assert_eq!(last_tx.pegout_idxs, vec![0, 1]);
        assert_eq!(last_tx.change_idxs, vec![2]);

        let block = create_block(vec![tx], bitcoin::BlockHash::all_zeros());
        pegout_scheduler.add_block(&block);

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
            txid: tx.txid(),
            tx: tx.clone(),
            pegout_idxs: vec![0],
            change_idxs: vec![1],
            created: SystemTime::now(),
            pegout_requests: pegouts,
        };

        let pegout_scheduler =
            PegoutScheduler::new(1, vec![tracked_tx], bitcoin::BlockHash::all_zeros(), db);
        let (last_tx_txid, last_tx) = pegout_scheduler.txs.clone().into_iter().next().unwrap();
        assert_eq!(last_tx_txid, tx.txid());
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
            pegout_scheduler.add_block(&block);
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
            txid: tx.txid(),
            tx: tx.clone(),
            pegout_idxs: vec![0],
            change_idxs: vec![1],
            created: SystemTime::now(),
            pegout_requests: pegouts,
        };

        let mut pegout_scheduler =
            PegoutScheduler::new(1, vec![tracked_tx], bitcoin::BlockHash::all_zeros(), db);
        assert_eq!(pegout_scheduler.txs.len(), 1);

        pegout_scheduler.un_track_tx(&tx.txid()).expect("untrack tx");
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
            relevant_txs: vec![tx.txid()],
            relevant_inputs: vec![],
        };
        pegout_scheduler.last_blocks.push_back(block);
        // Dont need to worry about last finalized
        pegout_scheduler.rollback_tip();
        assert_eq!(pegout_scheduler.txs.len(), 0);

        let pending_pegouts = db.get_pending_pegouts().unwrap();
        assert_eq!(pending_pegouts[0], pegouts[0]);
    }
}
