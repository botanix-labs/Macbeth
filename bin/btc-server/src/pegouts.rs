use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, SystemTime},
};

use bitcoin::{Amount, Block, BlockHash, OutPoint, Transaction, TxOut, Txid};
use bitcoincore_rpc::RpcApi;
use thiserror::Error;

use crate::database;

pub use reth_botanix_lib::peg_contract::PegoutId;


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


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PegoutRequest {
    pub id: PegoutId,
    /// The scriptpubkey of the pegout request.
    pub spk: bitcoin::ScriptBuf,
    /// The btc amount of pegout to deliver.
    pub value: Amount,
    /// Botanix block height this pegout was requested at.
    pub botanix_height: u64,
}

impl PegoutRequest {
    pub fn txout(&self) -> TxOut {
        TxOut {
            script_pubkey: self.spk.clone(),
            value: self.value,
        }
    }
}

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
    pub fn is_pegout(&self) -> bool {
        match self {
            Self::Pegout(_) => true,
            Self::Change => false,
        }
    }

    pub fn is_change(&self) -> bool {
        match self {
            Self::Pegout(_) => false,
            Self::Change => true,
        }
    }

    pub fn pegout_id(&self) -> Option<PegoutId> {
        match self {
            Self::Pegout(id) => Some(*id),
            Self::Change => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Tx {
    pub txid: Txid,
    pub tx: Transaction,
    /// Metadata for each output of the tx, by respective index.
    pub output_meta: Vec<OutputMeta>,
    pub created: SystemTime,
}

impl Tx {
    pub fn inputs(&self) -> impl Iterator<Item = OutPoint> + '_ {
        self.tx.input.iter().map(|i| i.previous_output)
    }

    /// Get all the pegouts of this tx. These are the outputs this tx delivers.
    /// I.e. all outputs that are not change outputs.
    pub fn pegouts(&self) -> impl Iterator<Item = (OutPoint, &TxOut, PegoutId)> {
        self.tx.output.iter().zip(self.output_meta.iter()).enumerate()
            .filter_map(|(i, (o, m))| m.pegout_id().map(|id| (OutPoint::new(self.txid, i as u32), o, id)))
    }

    /// Get all change outputs of this tx.
    #[allow(unused)]
    pub fn change(&self) -> impl Iterator<Item = (OutPoint, &TxOut)> {
        self.tx.output.iter().zip(self.output_meta.iter()).enumerate()
            .filter(|(_, (_, meta))| meta.is_change())
            .map(|(i, (o, _))| (OutPoint::new(self.txid, i as u32), o))
    }
}

struct BlockInfo {
    hash: BlockHash,
    relevant_txs: Vec<Txid>,
    relevant_inputs: Vec<OutPoint>,
}

pub struct PegoutManager {
    /// List of pending pegouts, ordered by botanix block height.
    pending_pegouts: HashMap<PegoutId, PegoutRequest>,

    /// The set of txs we are tracking.
    txs: HashMap<Txid, Tx>,
    txs_by_input: HashMap<OutPoint, Vec<Txid>>,
    txs_by_pegout: HashMap<PegoutId, Vec<Txid>>,
    /// The txs that are confirmed but not finalized yet.
    confirmed_txs: HashSet<Txid>,

    /// The number of blocks to track txs for.
    conf_window: u32,
    last_blocks: VecDeque<BlockInfo>,
    last_finalized: BlockHash,
}

impl PegoutManager {
    pub fn new(window: u32, txs: Vec<Tx>, last_finalized: BlockHash) -> PegoutManager {
        let mut ret = PegoutManager {
            pending_pegouts: HashMap::new(),
            txs: HashMap::with_capacity(txs.len()),
            txs_by_input: HashMap::with_capacity(txs.iter().map(|t| t.tx.input.len()).sum()),
            txs_by_pegout: HashMap::with_capacity(txs.iter().map(|t| t.pegouts().count()).sum()),
            confirmed_txs: HashSet::new(),
            conf_window: window,
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

    pub fn add_pegouts(
        &mut self,
        db: &database::Db,
        pegouts: Vec<PegoutRequest>,
    ) -> Result<(), database::Error> {
        for pegout in &pegouts {
            db.store_pending_pegout(pegout)?;
        }

        self.pending_pegouts.extend(pegouts.into_iter().map(|p| (p.id, p)));
        Ok(())
    }

    pub fn schedule_pegouts(&self) -> Vec<&PegoutRequest> {
        //TODO(stevenroose) be more intelligent here
        self.pending_pegouts.iter()
            .filter(|(id, _)| !self.txs_by_pegout.contains_key(id))
            .map(|(_id, p)| p)
            .collect()
    }

    pub fn last_finalized(&self) -> BlockHash {
        self.last_finalized
    }

    fn track_tx(&mut self, tx: Tx) {
        for input in tx.inputs() {
            self.txs_by_input.entry(input).or_default().push(tx.txid);
        }
        for (_utxo, _, id) in tx.pegouts() {
            self.txs_by_pegout.entry(id).or_default().push(tx.txid);
        }
        self.txs.insert(tx.txid, tx);
    }

    /// Add a new tx to the index for tracking.
    ///
    /// The pegouts should be in order, this will error if pegouts are not in order.
    pub fn add_tx(
        &mut self,
        tx: Transaction,
        pegouts: &[PegoutId],
        timestamp: SystemTime, //TODO(stevenroose) make height
    ) -> Result<&Tx, InternalPegoutRefError> {
        let pegouts = pegouts.iter()
            .map(|i| self.pending_pegouts.get(i).ok_or(InternalPegoutRefError))
            .collect::<Result<Vec<_>, _>>()?;
        let mut pegouts = &pegouts[..];

        // Tag each output with either the pegout id they are delivering or change.
        let output_meta = tx.output.iter().map(|out| {
            if !pegouts.is_empty() && out.script_pubkey == pegouts[0].spk && out.value == pegouts[0].value {
                let id = pegouts[0].id;
                pegouts = &pegouts[1..];
                OutputMeta::Pegout(id)
            } else {
                //TODO(stevenroose) maybe add additional checks for change?
                OutputMeta::Change
            }
        }).collect();
        if !pegouts.is_empty() {
            return Err(InternalPegoutRefError);
        }
        let txid = tx.txid();
        self.track_tx(Tx {
            created: timestamp,
            txid,
            tx,
            output_meta,
        });
        Ok(self.txs.get(&txid).expect("just put it in"))
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

    fn rollback_tip(&mut self) {
        assert!(!self.last_blocks.is_empty());
        let drop = self.last_blocks.pop_back().unwrap();
        for tx in drop.relevant_txs {
            self.confirmed_txs.remove(&tx);
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
                self.confirmed_txs.insert(txid);
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
            "Syncing PegoutMgr: last={}:{}, cp={}:{}",
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

            if self.last_blocks.len() == self.conf_window as usize {
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
#[error("invalid pegouts passed into method")]
pub struct InternalPegoutRefError;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid pegouts passed into method")]
    InvalidPegouts,
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
