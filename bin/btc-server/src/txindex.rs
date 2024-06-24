use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, SystemTime},
};

use bitcoin::{Block, BlockHash, OutPoint, Transaction, TxOut, Txid};
use bitcoincore_rpc::RpcApi;
use thiserror::Error;

use crate::database;

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

#[derive(Debug, Serialize, Deserialize)]
pub struct Tx {
    pub txid: Txid,
    pub tx: Transaction,
    pub pegout_idxs: Vec<usize>,
    pub change_idxs: Vec<usize>,
    pub created: SystemTime,
}

impl Tx {
    pub fn inputs<'a>(&'a self) -> impl Iterator<Item = OutPoint> + 'a {
        self.tx.input.iter().map(|i| i.previous_output)
    }

    /// Get all the pegouts of this tx. These are the outputs this tx delivers.
    /// I.e. all outputs that are not change outputs.
    pub fn pegouts<'a>(
        &'a self,
    ) -> impl Iterator<Item = (OutPoint, &'a TxOut)> + ExactSizeIterator + 'a {
        self.pegout_idxs.iter().map(|i| {
            let point = OutPoint::new(self.txid, *i as u32);
            let output = &self.tx.output[*i];
            (point, output)
        })
    }

    /// Get all change outputs of this tx.
    #[allow(unused)]
    pub fn change<'a>(
        &'a self,
    ) -> impl Iterator<Item = (OutPoint, &'a TxOut)> + ExactSizeIterator + 'a {
        self.change_idxs.iter().map(|i| {
            let point = OutPoint::new(self.txid, *i as u32);
            let output = &self.tx.output[*i];
            (point, output)
        })
    }
}

struct BlockInfo {
    hash: BlockHash,
    relevant_txs: Vec<Txid>,
    relevant_inputs: Vec<OutPoint>,
}

pub struct TxIndex {
    /// The number of blocks to track txs for.
    window: u32,

    /// The set of txs we are tracking.
    txs: HashMap<Txid, Tx>,
    txs_by_input: HashMap<OutPoint, Vec<Txid>>,
    txs_by_pegout: HashMap<TxOut, Vec<Txid>>,
    /// The txs that are confirmed but not finalized yet.
    confirmed: HashSet<Txid>,

    last_blocks: VecDeque<BlockInfo>,
    last_finalized: BlockHash,
}

impl TxIndex {
    pub fn new(window: u32, txs: Vec<Tx>, last_finalized: BlockHash) -> TxIndex {
        let mut ret = TxIndex {
            window,
            txs: HashMap::with_capacity(txs.len()),
            txs_by_input: HashMap::with_capacity(txs.iter().map(|t| t.tx.input.len()).sum()),
            txs_by_pegout: HashMap::with_capacity(txs.iter().map(|t| t.pegouts().len()).sum()),
            confirmed: HashSet::new(),
            last_blocks: VecDeque::with_capacity(window as usize),
            last_finalized,
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

    pub fn last_finalized(&self) -> BlockHash {
        self.last_finalized
    }

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
    ///
    /// Panics if [pegouts] isn't a strict subset of the transaction's outputs.
    pub fn add_tx(&mut self, tx: Transaction, pegouts: &[TxOut], timestamp: SystemTime) -> &Tx {
        let mut idxs = (0..tx.output.len()).collect::<Vec<_>>();
        let pegout_idxs = {
            let mut ret = Vec::with_capacity(idxs.len());
            for pegout in pegouts {
                let idx = idxs
                    .iter()
                    .find(|i| tx.output[**i] == *pegout)
                    .expect("tx doesn't contain all pegouts");
                ret.push(*idx);
                idxs.remove(*idx);
            }
            ret
        };
        let txid = tx.txid();
        self.track_tx(Tx {
            created: timestamp,
            change_idxs: idxs, // leftover not pegouts is change
            txid,
            tx,
            pegout_idxs,
        });
        self.txs.get(&txid).expect("just put it in")
    }

    /// Get all input utxos that are spent by pending txs.
    pub fn pending_inputs(&self) -> HashSet<OutPoint> {
        let mut ret = HashSet::with_capacity(self.txs.len() * 3);
        for tx in self.txs.values() {
            ret.extend(tx.inputs());
        }
        ret.shrink_to_fit();
        ret
    }

    /// Get all utxos that are created by pending txs but are already confirmed.
    pub fn pending_confirmed_utxos(&self) -> HashSet<OutPoint> {
        let mut ret = HashSet::with_capacity(self.txs.len() * 3);
        for tx in self.txs.values() {
            if self.confirmed.contains(&tx.txid) {
                for vout in 0..tx.tx.output.len() {
                    ret.insert(OutPoint::new(tx.txid, vout as u32));
                }
            }
        }
        ret.shrink_to_fit();
        ret
    }

    fn rollback_tip(&mut self) {
        assert!(!self.last_blocks.is_empty());
        let drop = self.last_blocks.pop_back().unwrap();
        for tx in drop.relevant_txs {
            self.confirmed.remove(&tx);
        }
    }

    fn finalize_block(
        &mut self,
        finalize_utxo: &mut impl FnMut(database::Utxo) -> Result<(), database::Error>,
        block: &BlockInfo,
    ) -> Result<(), database::Error> {
        // To make sure we only update the index when the db is also synced,
        // first try store the new finalized UTXOs to the db, then update the index.
        let mut all_inputs = block.relevant_inputs.iter().copied().collect::<HashSet<_>>();
        for txid in &block.relevant_txs {
            let tx = self.txs.get(txid).expect("corrupt db");

            for (idx, output) in tx.tx.output.iter().enumerate() {
                finalize_utxo(database::Utxo {
                    outpoint: OutPoint::new(tx.txid, idx as u32),
                    output: output.clone(),
                    eth_address: None,
                })?;
            }
            all_inputs.extend(tx.tx.input.iter().map(|i| i.previous_output));
        }

        // Now that it's all in the db, we can apply changes here.
        for input in all_inputs {
            if let Some(tx) = self.txs.remove(&input.txid) {
                info!("Dropping tx that conflicts with finalized tx: {:?}", tx);
            }
        }
        for txid in &block.relevant_txs {
            self.txs.remove(txid);
        }

        Ok(())
    }

    /// Adds a new block to the chain.
    ///
    /// Updates the [SyncResult] with the data from newly finalized blocks.
    fn add_block(&mut self, block: &Block) {
        let hash = block.block_hash();
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
                self.confirmed.insert(txid);
            } else {
                for input in &tx.input {
                    if let Some(conflicts) = self.txs_by_input.get(&input.previous_output) {
                        warn!(
                            "Tx confirmed that conflicts with one of our txs: \
                            new={}, ours={:?}, block={}:{}",
                            txid, conflicts, height, hash,
                        );
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
        mut finalize_utxo: impl FnMut(database::Utxo) -> Result<(), database::Error>,
    ) -> Result<(), SyncError> {
        info!(
            "Syncing TxIndex: last={}:{}, cp={}:{}",
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
                        // our window has taken place. We can't do anything at this point.
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
                break;
            }

            let block = bitcoind.get_block(&hash)?;
            self.add_block(&block);

            if self.last_blocks.len() == self.window as usize {
                let deep = self.last_blocks.pop_front().unwrap();
                self.finalize_block(&mut finalize_utxo, &deep)?;
                self.last_finalized = deep.hash;
            }
        }

        if self.last_finalized == checkpoint {
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
    #[error("the node isn't synced yet")]
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
