//! The purpose of this module is to provide a bridge between the CometBFT and the EVM application
//! state
use alloy_rpc_types_engine::ForkchoiceState;
use reth_chain_state::ExecutedBlock;
use reth_chainspec::ChainSpec;
use reth_db::{
    models::{self, SnapshotSync, SnapshotSyncId},
    Database, DatabaseEnv,
};
use reth_provider::{
    providers::BlockchainProvider2, BlockWriter, CanonChainTracker, ExecutionOutcome, StagedHeader,
};
use reth_trie::{updates::TrieUpdates, StateRoot};
use reth_trie_db::DatabaseStateRoot;
use std::{
    error::Error,
    io::{self},
    sync::{Arc, RwLock},
};
use thiserror::Error;
use tokio::sync::Mutex;

use botanix_btc_wallet::bitcoind::BitcoindFactory;
use botanix_data_parser::DataParser;
use reth_basic_payload_builder::{BuildArguments, PayloadConfig};
use reth_consensus::{Consensus, ConsensusError, InvalidAggregatedPublicKeyError};
use reth_consensus_common::utils::unix_timestamp;
use reth_ethereum_payload_builder::default_ethereum_payload_builder;
use reth_evm::execute::BlockExecutorProvider;

use botanix_comet_bft_rpc::HttpCometBFTRpcClientFactory;
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_primitives::{
    botanix::block_with_peg::SealedBlockWithPeg, header_ext::HeaderExt, Address, BlockHash,
    BlockNumber, BlockWithSenders, SealedBlock, B256,
};
use reth_provider::{
    BlockReaderIdExt, CanonStateNotification, Chain, ProviderError, ProviderFactory,
    SnapshotReader, SnapshotWriter, StateProviderFactory,
};
use reth_revm::primitives::FixedBytes;
use reth_rpc_types::{engine::PayloadAttributes, BlockId};
use reth_tasks::{TaskExecutor, TaskSpawner};
use reth_transaction_pool::TransactionPool;
use schnellru::{ByLength, LruMap};

use tendermint_abci::{Application, ServerBuilder};
use tendermint_proto::{
    abci::{
        ExecTxResult, RequestPrepareProposal, RequestProcessProposal, ResponseCommit,
        ResponsePrepareProposal, ResponseProcessProposal,
    },
    v0_38::abci::{
        RequestApplySnapshotChunk, RequestCheckTx, RequestFinalizeBlock, RequestInfo,
        RequestInitChain, RequestLoadSnapshotChunk, RequestOfferSnapshot,
        ResponseApplySnapshotChunk, ResponseCheckTx, ResponseFinalizeBlock, ResponseInfo,
        ResponseInitChain, ResponseListSnapshots, ResponseLoadSnapshotChunk, ResponseOfferSnapshot,
        Snapshot,
    },
};

impl From<&Snapshot> for SnapshotSyncStateLock {
    fn from(snapshot: &Snapshot) -> Self {
        let mut s = SnapshotSyncStateLock::default();
        s.set_snapshot_height(snapshot.height)
            .set_snapshot_chunks(snapshot.chunks as u64)
            .set_snapshot_format(snapshot.format as u64)
            .set_snapshot_hash(snapshot.hash.clone());
        s
    }
}

/// Offer Snapshot Result
enum SnapshotOfferResult {
    Unknown = 0, // Unknown result, abort all snapshot restoration
    Accept = 1,  // Snapshot is accepted, start applying chunks.
    #[allow(dead_code)]
    Abort = 2, // Abort snapshot restoration, and don't try any other snapshots.
    #[allow(dead_code)]
    Reject = 3, // Reject this specific snapshot, try others.
    #[allow(dead_code)]
    RejectFormat = 4, // Reject all snapshots with this `format`, try others.
    #[allow(dead_code)]
    RejectSender = 5, // Reject all snapshots from all senders of this snapshot, try others.
}

/// Apply Snapshot Results
pub enum ApplySnapshotResult {
    /// Unknown result, abort all snapshot restoration
    Unknown = 0,
    /// Chunk successfully accepted
    Accept = 1,
    /// Abort all snapshot restoration
    Abort = 2,
    /// Retry chunk (combine with refetch and reject)
    Retry = 3,
    /// Retry snapshot (combine with refetch and reject)
    RetrySnapshot = 4,
    /// Reject this snapshot, try others
    RejectSnapshot = 5,
}

use super::proto_debug::{
    RequestApplySnapshotChunkTruncatedDebug, RequestFinalizeBlockTruncatedDebug,
    RequestProcessProposalTruncatedDebug, ResponseLoadSnapshotChunkTruncatedDebug,
    ResponsePrepareProposalTruncatedDebug,
};
use crate::{
    bitcoin_checkpoint::BitcoinCheckpointsChain,
    comet_bft::{
        non_deterministic_data::{NonDeterministicData, VERSION_1 as LATEST_NDD_VERSION},
        utils::transactions_signed_from_bytes,
    },
    excecution_utils::authority_execution_utils::{batch_execute, build_and_execute},
    metrics::AuthorityMetrics,
    snapshot_manager::{SnapshotManagerError, SnapshotManagerStateLock},
    utils::{get_staged_pegins_from_pegin_meta, get_staged_pegouts_from_pegout_data},
    AuthorityConsensus, Storage,
};
use tracing::{debug, error, info, instrument, trace, warn};

/// Consts
const SUCCESS: u32 = 0;
const _ERROR: u32 = 1;

// https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#verifystatus
const _VERIFY_UNKNOWN: i32 = 0;
const VERIFY_ACCEPTED: i32 = 1;
const VERIFY_REJECT: i32 = 2;

// Version
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Snapshot Sync State Lock
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SnapshotSyncStateLock {
    snapshot_height: u64,
    snapshot_hash: prost::bytes::Bytes,
    snapshot_chunks: u64,
    snapshot_format: u64,
}

impl SnapshotSyncStateLock {
    /// Set snapshot height
    pub fn set_snapshot_height(&mut self, snapshot_id: u64) -> &mut Self {
        self.snapshot_height = snapshot_id;
        self
    }

    /// Set snapshot hash
    pub fn set_snapshot_hash(&mut self, snapshot_hash: prost::bytes::Bytes) -> &mut Self {
        self.snapshot_hash = snapshot_hash;
        self
    }

    /// Set snapshot chunks
    pub fn set_snapshot_chunks(&mut self, snapshot_chunks: u64) -> &mut Self {
        self.snapshot_chunks = snapshot_chunks;
        self
    }

    /// Set snapshot format
    pub fn set_snapshot_format(&mut self, snapshot_format: u64) -> &mut Self {
        self.snapshot_format = snapshot_format;
        self
    }

    /// Get snapshot chunks
    pub fn get_snapshot_height(&self) -> u64 {
        self.snapshot_height
    }

    /// Get snapshot hash
    pub fn get_snapshot_hash(&self) -> &[u8] {
        &self.snapshot_hash
    }

    /// Get snapshot chunks
    pub fn get_snapshot_chunks(&self) -> u64 {
        self.snapshot_chunks
    }

    /// Get snapshot format
    pub fn get_snapshot_format(&self) -> u64 {
        self.snapshot_format
    }
}

/// Block with execution context, trie updates and botanix peg data
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockWithContext {
    /// The sealed block with peg data
    pub sealed_block_with_peg: SealedBlockWithPeg,
    /// The execution outcome
    pub exec_outcome: ExecutionOutcome,
    /// The trie updates
    pub trie_updates: TrieUpdates,
}

/// ABCI client builder
#[derive(Clone)]
pub struct ABCIClientBuilder<EF, BF, DB> {
    storage: Storage<EF, BF, DB>,
    bitcoin_checkpoints: Arc<BitcoinCheckpointsChain>,
    authority_consensus: AuthorityConsensus,
    cbft_rpc_client_factory: HttpCometBFTRpcClientFactory,
    is_fed_node: bool,
    metrics: Arc<AuthorityMetrics>,
    compressor: DataParser,
    task_executor: TaskExecutor,
    abci_driver_tx: tokio::sync::mpsc::Sender<ABCIDriverMessage>,
    provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
    snapshot_manager_state_lock: Arc<RwLock<SnapshotManagerStateLock>>,
    snapshot_sync_state_lock: Option<Arc<RwLock<SnapshotSyncStateLock>>>,
    snapshot_format: u32,
    block_fee_recipient_address: Option<reth_primitives::Address>,
    blockchain_db: BlockchainProvider2<Arc<DatabaseEnv>>,
}

impl<EF, BF, DB> ABCIClientBuilder<EF, BF, DB>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + Clone
        + SnapshotReader
        + SnapshotWriter
        + CanonChainTracker
        + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    BF: BitcoindFactory + Clone + Unpin + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        storage: Storage<EF, BF, DB>,
        bitcoin_checkpoints: Arc<BitcoinCheckpointsChain>,
        authority_consensus: AuthorityConsensus,
        cbft_rpc_client_factory: HttpCometBFTRpcClientFactory,
        is_fed_node: bool,
        metrics: Arc<AuthorityMetrics>,
        task_executor: TaskExecutor,
        compressor: DataParser,
        abci_driver_tx: tokio::sync::mpsc::Sender<ABCIDriverMessage>,
        provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
        snapshot_manager_state_lock: Arc<RwLock<SnapshotManagerStateLock>>,
        snapshot_format: u32,
        block_fee_recipient_address: Option<reth_primitives::Address>,
    ) -> Self {
        let latest_sealed_header = storage
            .client
            .latest_header()
            .ok()
            .flatten()
            .unwrap_or_else(|| storage.chain_spec.sealed_genesis_header());
        let blockchain_db =
            BlockchainProvider2::with_latest(provider_factory.clone(), latest_sealed_header)
                .expect("blockchain db to exist");

        Self {
            storage,
            bitcoin_checkpoints,
            authority_consensus,
            cbft_rpc_client_factory,
            is_fed_node,
            metrics,
            task_executor,
            abci_driver_tx,
            provider_factory,
            compressor,
            snapshot_manager_state_lock,
            snapshot_sync_state_lock: Some(Arc::new(RwLock::new(SnapshotSyncStateLock::default()))),
            snapshot_format,
            block_fee_recipient_address,
            blockchain_db,
        }
    }

    /// Starts the abci client server
    pub async fn start_server<Pool: TransactionPool + Clone + 'static>(
        &self,
        task_executor: &impl TaskSpawner,
        tx_pool: Pool,
        abci_host: String,
        abci_port: u16,
    ) -> Result<(), tendermint_abci::Error> {
        let app = ABCIClient::new(
            self.storage.clone(),
            tx_pool,
            self.bitcoin_checkpoints.clone(),
            self.abci_driver_tx.clone(),
            self.cbft_rpc_client_factory.clone(),
            self.authority_consensus.clone(),
            self.is_fed_node,
            self.metrics.clone(),
            self.compressor.clone(),
            self.task_executor.clone(),
            self.provider_factory.clone(),
            self.snapshot_manager_state_lock.clone(),
            self.snapshot_sync_state_lock.clone(),
            self.snapshot_format,
            self.block_fee_recipient_address,
            self.blockchain_db.clone(),
        );

        let server_builder = ServerBuilder::default();
        // server will always bind to default address
        // CometBFT will always run on the same machine and container
        let server = server_builder.bind(format!("{abci_host}:{abci_port}"), app)?;

        if self.is_fed_node {
            loop {
                let storage = self.storage.inner.read().await;
                if storage.aggregate_public_key.is_some() {
                    info!(
                        "Aggregate public key is stored in the storage continuing to start ABCI server"
                    );
                    break;
                }
                info!(
                    "Waiting for aggregate public key to be stored in the storage before starting ABCI server"
                );
                drop(storage);
                tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
            }
        }

        task_executor.spawn_critical(
            "ABCI Client",
            Box::pin(async move {
                // we should panic here if cannot launch the ABCI server
                server.listen().expect("to start server");
            }),
        );
        Ok(())
    }
}

#[derive(Debug, Error)]
enum PayloadBuilderError {
    #[error("Provider failed db read: {0}")]
    ProviderError(#[from] ProviderError),
    #[error("Latest header does not exist")]
    LatestHeaderDoesNotExist,
    #[error("Latest block does not exist")]
    LatestBlockDoesNotExist,
    #[error("Parent block does not exist")]
    ParentBlockDoesNotExist,
}

struct BlockCache {
    /// Cached blocks, inserted after execution in either `process_proposal` or
    /// `finalize_block`.
    cache: LruMap<BlockHash, BlockWithContext>,
    /// The finalized block hash tracked by `finalize_block`, and then consumed
    /// by `commit`.
    tracked_final: Option<BlockHash>,
}

#[derive(Clone)]
pub(crate) struct ABCIClient<EF, BF, DB, Pool> {
    storage: Storage<EF, BF, DB>,
    pool: Pool,
    bitcoin_checkpoints: Arc<BitcoinCheckpointsChain>,
    block_cache: Arc<RwLock<BlockCache>>,
    driver_tx: tokio::sync::mpsc::Sender<ABCIDriverMessage>,
    #[allow(dead_code)]
    cbft_rpc_provider: HttpCometBFTRpcClientFactory,
    authority_consensus: AuthorityConsensus,
    is_fed_node: bool,
    metrics: Arc<AuthorityMetrics>,
    task_executor: TaskExecutor,
    provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
    compressor: DataParser,
    snapshot_manager_state_lock: Arc<RwLock<SnapshotManagerStateLock>>,
    snapshot_sync_state_lock: Option<Arc<RwLock<SnapshotSyncStateLock>>>,
    snapshot_format: u32,
    block_fee_recipient_address: Option<reth_primitives::Address>,
    blockchain_db: BlockchainProvider2<Arc<DatabaseEnv>>,
}

impl<EF, BF, DB, Pool> ABCIClient<EF, BF, DB, Pool>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + Clone
        + SnapshotReader
        + SnapshotWriter
        + CanonChainTracker
        + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    BF: BitcoindFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Clone + 'static,
{
    #[allow(clippy::too_many_arguments)]
    fn new(
        storage: Storage<EF, BF, DB>,
        pool: Pool,
        bitcoin_checkpoints: Arc<BitcoinCheckpointsChain>,
        driver_tx: tokio::sync::mpsc::Sender<ABCIDriverMessage>,
        cbft_rpc_provider: HttpCometBFTRpcClientFactory,
        authority_consensus: AuthorityConsensus,
        is_fed_node: bool,
        metrics: Arc<AuthorityMetrics>,
        compressor: DataParser,
        task_executor: TaskExecutor,
        provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
        snapshot_manager_state_lock: Arc<RwLock<SnapshotManagerStateLock>>,
        snapshot_sync_state_lock: Option<Arc<RwLock<SnapshotSyncStateLock>>>,
        snapshot_format: u32,
        block_fee_recipient_address: Option<reth_primitives::Address>,
        blockchain_db: BlockchainProvider2<Arc<DatabaseEnv>>,
    ) -> Self {
        // Saving the last 5 blocks that were proposed
        let block_cache = Arc::new(RwLock::new(BlockCache {
            cache: LruMap::new(ByLength::new(5)),
            tracked_final: None,
        }));

        Self {
            storage: storage.clone(),
            pool,
            bitcoin_checkpoints,
            // Saving the last 5 blocks that were proposed
            block_cache,
            driver_tx,
            cbft_rpc_provider,
            authority_consensus,
            is_fed_node,
            metrics,
            compressor,
            task_executor,
            provider_factory,
            snapshot_manager_state_lock,
            snapshot_sync_state_lock,
            snapshot_format,
            block_fee_recipient_address,
            blockchain_db,
        }
    }

    /// Returns the payload builder config
    /// this method will block and wait for the storage lock
    fn payload_builder_arguments(
        &self,
    ) -> Result<PayloadConfig<EthPayloadBuilderAttributes>, PayloadBuilderError> {
        let client = self.storage.client.clone();
        let chain_spec = self.storage.chain_spec.clone();

        let best_header =
            client.latest_header()?.ok_or(PayloadBuilderError::LatestHeaderDoesNotExist)?;
        let best_block = BlockReaderIdExt::block_by_id(&client, BlockId::latest())?
            .ok_or(PayloadBuilderError::LatestBlockDoesNotExist)?
            .seal(best_header.hash());

        let parent_block =
            BlockReaderIdExt::block_by_id(&client, BlockId::hash(best_header.parent_hash))?
                .ok_or(PayloadBuilderError::ParentBlockDoesNotExist)?
                .seal(best_header.parent_hash);

        let payload_attributes = PayloadAttributes {
            // Attributes here dont really matter
            // We just want to build a payload with the best txs
            // TODO: Why we don't use block time here?
            timestamp: unix_timestamp(),
            prev_randao: FixedBytes::<32>::random(),
            suggested_fee_recipient: Address::ZERO,
            withdrawals: None,
            parent_beacon_block_root: parent_block.parent_beacon_block_root,
        };

        let payload_builder_attributes =
            EthPayloadBuilderAttributes::new(best_block.hash(), payload_attributes);

        Ok(PayloadConfig::new(
            Arc::new(best_block),
            reth_primitives::Bytes::default(),
            payload_builder_attributes,
            chain_spec,
        ))
    }

    pub(crate) fn non_deterministic_data(&self) -> Result<NonDeterministicData, ConsensusError> {
        let aggregate_public_key = self.aggregate_public_key()?;
        let block_fee_recipient_address = self
            .block_fee_recipient_address
            .ok_or(ConsensusError::MissingBlockFeeRecipientAddress)?;

        let ndd = NonDeterministicData::new(
            self.bitcoin_blockhash()?,
            aggregate_public_key,
            block_fee_recipient_address,
        );

        Ok(ndd)
    }

    pub(crate) fn serialize_non_deterministic_data_to_bytes(
        &self,
        ndd: NonDeterministicData,
    ) -> Result<prost::bytes::Bytes, ConsensusError> {
        let ndd_bytes = prost::bytes::Bytes::copy_from_slice(
            ndd.serialize()
                .map_err(|_| ConsensusError::NonDeterministicDataDeserialize)?
                .as_slice(),
        );

        Ok(ndd_bytes)
    }

    pub(crate) fn validate_block(&self, block: &SealedBlock) -> ResponseProcessProposal {
        // validate_block_post_execution() is called when inserting the block (ABCIDriver)
        match self.authority_consensus.validate_block_pre_execution(block) {
            Ok(_) => {}
            Err(e) => {
                error!("Error in validate_block_pre_execution(): {:?}", e);
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        }

        // standard header validation
        match self.authority_consensus.validate_header(&block.header) {
            Ok(_) => {}
            Err(e) => {
                error!("Error in validate_header(): {:?}", e);
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        }

        // poa validation
        let agg_pk = match self.aggregate_public_key() {
            Ok(pk) => pk,
            Err(e) => {
                error!("Error getting aggregate public key: {:?}", e);
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        };

        match self.authority_consensus.validate_header_standalone(
            block.header(),
            self.storage.genesis_authorities.as_slice(),
            Some(&agg_pk),
        ) {
            Ok(_) => {}
            Err(e) => {
                error!("Error in validate_header_standalone(): {:?}", e);
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        }

        ResponseProcessProposal { status: VERIFY_ACCEPTED }
    }

    pub(crate) fn aggregate_public_key(&self) -> Result<secp256k1::PublicKey, ConsensusError> {
        match self.storage.inner.blocking_read().aggregate_public_key {
            Some(pk) => Ok(pk),
            None => Err(ConsensusError::InvalidAggregatedPublicKey(
                InvalidAggregatedPublicKeyError::MissingAggregatedPublicKey,
            )),
        }
    }

    pub(crate) fn bitcoin_blockhash(&self) -> Result<bitcoin::BlockHash, ConsensusError> {
        self.bitcoin_checkpoints
            .strong()
            .ok_or(ConsensusError::MissingBitcoinCheckpoint)
            .map(|checkpoint| checkpoint.hash)
    }

    pub(crate) fn application_hash(
        &self,
        db: &impl BlockReaderIdExt,
    ) -> Result<prost::bytes::Bytes, ConsensusError> {
        let header = db
            .latest_header()
            .map_err(ConsensusError::Provider)?
            .ok_or(ConsensusError::LatestHeaderMissing)?;
        Ok(prost::bytes::Bytes::copy_from_slice(&header.hash().0))
    }
}

impl<EF, BF, DB, Pool> ABCIClient<EF, BF, DB, Pool>
where
    DB: BlockReaderIdExt + StateProviderFactory + SnapshotReader + SnapshotWriter + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    BF: BitcoindFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Clone + 'static,
{
    fn create_new_snapshot_sync(
        &self,
        block_id: BlockNumber,
        snapshot_hash: B256,
        total_chunks: u64,
        format: u64,
    ) -> Result<u64, SnapshotManagerError> {
        let provider_rw = self.provider_factory.provider_rw()?;
        let snapshot_sync_id =
            provider_rw.create_new_snapshot_sync(block_id, snapshot_hash, total_chunks, format)?;
        provider_rw.commit()?;
        Ok(snapshot_sync_id)
    }

    fn update_snapshot_sync(
        &self,
        snapshot_sync_id: SnapshotSyncId,
        updated_snapshot: SnapshotSync,
    ) -> Result<(), SnapshotManagerError> {
        let provider_rw = self.provider_factory.provider_rw()?;
        provider_rw.update_snapshot_sync(snapshot_sync_id, updated_snapshot)?;
        provider_rw.commit()?;
        Ok(())
    }
}

impl<EF, BF, DB, Pool> Application for ABCIClient<EF, BF, DB, Pool>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + Clone
        + SnapshotReader
        + SnapshotWriter
        + CanonChainTracker
        + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    BF: BitcoindFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Clone + 'static,
{
    // docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#init_chain
    // Panic! on an error. Proceeding when the chain can't be initialized will lead
    // to unexpected behavior.
    #[instrument(level = "trace", ret, skip(self, request))]
    fn init_chain(&self, request: RequestInitChain) -> ResponseInitChain {
        let execution_start_time = std::time::Instant::now();
        trace!("request={:?}", request);

        // check chain ids match
        let cometbft_chain_id = match request.chain_id.parse::<u64>() {
            Ok(chain_id) => chain_id,
            Err(e) => {
                panic!("Error parsing cometbft chain id: {:?}", e);
            }
        };
        assert_eq!(self.storage.chain_spec.chain.id(), cometbft_chain_id, "Chain ID mismatch");

        let client = self.storage.client.clone();
        let app_hash = match self.application_hash(&client) {
            Ok(app_hash) => app_hash,
            Err(e) => {
                panic!("Error getting application hash: {:?}", e);
            }
        };

        let execution_time = execution_start_time.elapsed().as_secs_f32();

        info!(
            app_hash = hex::encode(&app_hash),
            chain_id = cometbft_chain_id,
            execution_time,
            "Chain {cometbft_chain_id} is initialized in {execution_time} secs",
        );

        ResponseInitChain { app_hash, ..Default::default() }
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#info
    #[instrument(level = "trace", ret, skip(self, request))]
    fn info(&self, request: RequestInfo) -> ResponseInfo {
        trace!("request={:?}", request);

        let client = self.storage.client.clone();

        let latest_header = match client.latest_header() {
            Ok(Some(header)) => header,
            Ok(None) => {
                error!("No latest header found");
                return ResponseInfo { data: String::default(), ..Default::default() };
            }
            Err(e) => {
                error!("Error getting latest header: {:?}", e);
                return ResponseInfo { data: String::default(), ..Default::default() };
            }
        };

        let last_block_app_hash = match self.application_hash(&client) {
            Ok(application_hash) => application_hash,
            Err(e) => {
                error!("Error getting application hash: {:?}", e);
                return ResponseInfo { data: String::default(), ..Default::default() };
            }
        };

        ResponseInfo {
            data: String::default(),
            version: VERSION.to_string(),
            app_version: 1,
            last_block_height: latest_header.number as i64,
            last_block_app_hash,
        }
    }

    /// https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#listsnapshots
    #[instrument(level = "trace", ret, skip(self))]
    fn list_snapshots(&self) -> ResponseListSnapshots {
        trace!("list_snapshots request");

        let client = self.storage.client.clone();
        match client.get_snapshots() {
            Ok(snapshots) => {
                // ensure no historical sync is ongoing
                let snapshot_manager_state_lock = match self.snapshot_manager_state_lock.read() {
                    Ok(snapshot_manager_state_lock) => snapshot_manager_state_lock,
                    Err(e) => {
                        error!("Error getting a snapshot state lock: {:?}", e);
                        return ResponseListSnapshots { snapshots: vec![] };
                    }
                };

                if snapshot_manager_state_lock.is_syncing_history() {
                    drop(snapshot_manager_state_lock);
                    debug!("Historical syncing ongoing. No snapshots available yet ...");
                    return ResponseListSnapshots { snapshots: vec![] };
                }
                // filter out the snapshot that is the same as the current block as we might not be
                // ready having all the chunks yet
                let resp = snapshots
                    .into_iter()
                    .filter(|s| s.height() != snapshot_manager_state_lock.get_block_id())
                    .fold(ResponseListSnapshots { snapshots: vec![] }, |mut acc, snapshot| {
                        acc.snapshots.push(Snapshot {
                            height: snapshot.height(),
                            format: self.snapshot_format,
                            chunks: snapshot.chunk_ids().len() as u32,
                            hash: snapshot.get_hash().to_vec().into(),
                            metadata: prost::bytes::Bytes::new(),
                        });
                        acc
                    });
                drop(snapshot_manager_state_lock);

                if tracing::enabled!(tracing::Level::TRACE) {
                    trace!(
                        "Responded with snapshots for block heights {:?}",
                        resp.snapshots.iter().map(|s| s.height).collect::<Vec<_>>()
                    );
                }

                resp
            }
            Err(e) => {
                error!("Error getting snapshots from db: {:?}", e);
                ResponseListSnapshots { snapshots: vec![] }
            }
        }
    }

    /// https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#offersnapshot
    #[instrument(level = "trace", ret, skip(self, request), fields(height))]
    fn offer_snapshot(&self, request: RequestOfferSnapshot) -> ResponseOfferSnapshot {
        trace!("request={:?}", request);

        let Some(snapshot) = request.snapshot else {
            error!("received empty snapshot");

            return ResponseOfferSnapshot { result: SnapshotOfferResult::Unknown as i32 };
        };

        tracing::Span::current().record("height", snapshot.height);

        // ensure no historical sync is ongoing
        let snapshot_manager_state_lock = match self.snapshot_manager_state_lock.read() {
            Ok(snapshot_manager_state_lock) => snapshot_manager_state_lock,
            Err(e) => {
                error!("Error getting a snapshot state lock: {:?}", e);
                return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
            }
        };

        if snapshot_manager_state_lock.is_syncing_history() {
            drop(snapshot_manager_state_lock);
            info!("Historical syncing ongoing. No snapshots available yet ...");
            return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
        }

        // some other node is offering us a snapshot - we need to validate here if we want to accept
        // it
        if request.app_hash.is_empty() {
            warn!("Received empty app hash in offer_snapshot request, rejecting snapshot");
            return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
        }

        let client = self.storage.client.clone();
        let application_hash = match self.application_hash(&client) {
            Ok(application_hash) => application_hash,
            Err(e) => {
                error!("Error getting application hash: {:?}", e);
                return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
            }
        };

        if application_hash == request.app_hash {
            warn!("Application hash matches, snapshot must have already been applied, rejecting snapshot");
            return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
        }

        if snapshot.format != self.snapshot_format {
            warn!("Received snapshot format is not supported, rejecting snapshot");
            return ResponseOfferSnapshot { result: SnapshotOfferResult::RejectFormat as i32 };
        }

        if snapshot.chunks == 0 {
            warn!("Received snapshot has no chunks, rejecting snapshot");
            return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
        }

        if snapshot.hash == prost::bytes::Bytes::default() {
            warn!("Received snapshot has no hash (empty bytes), rejecting snapshot");
            return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
        }

        // read the lock and make sure we are not already syncing the snapshot we are being
        // offered
        if let Some(snapshot_sync_state_lock) = self.snapshot_sync_state_lock.as_ref() {
            let snapshot_sync_state_lock = match snapshot_sync_state_lock.read() {
                Ok(snapshot_sync_state_lock) => snapshot_sync_state_lock,
                Err(e) => {
                    error!("Error getting a snapshot state lock: {:?}", e);
                    return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
                }
            };

            // we are already syncing the this snapshot
            if (*snapshot_sync_state_lock).eq(&SnapshotSyncStateLock::from(&snapshot)) {
                drop(snapshot_sync_state_lock);
                // since the lock is still on the currently accepted snapshot, we must return
                // accept
                return ResponseOfferSnapshot { result: SnapshotOfferResult::Accept as i32 };
            }
        }

        // check that we should not have the block at height already
        if client.block_by_id(BlockId::number(snapshot.height)).ok().flatten().is_some() {
            warn!("Block at height {:?} already exists, rejecting snapshot", snapshot.height);
            return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
        }

        // get the latest header
        let latest_header = match client.latest_header() {
            Ok(Some(header)) => header,
            Ok(None) => {
                error!("No latest header found");
                return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
            }
            Err(e) => {
                error!("Error getting latest header: {:?}", e);
                return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
            }
        };

        // check that the latest header is less than the snapshot height
        if latest_header.header().number > snapshot.height {
            warn!(
                "Latest header height {:?} is greater than snapshot height {:?}, rejecting snapshot",
                latest_header.header().number, snapshot.height
            );
            return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
        }

        // ensure that the last sync lock is less than the newly offered height
        if let Some(snapshot_sync_state_lock) = self.snapshot_sync_state_lock.as_ref() {
            let snapshot_sync_state_lock_height = match snapshot_sync_state_lock.read() {
                Ok(snapshot_sync_state_lock_height) => snapshot_sync_state_lock_height,
                Err(e) => {
                    error!("Error getting a snapshot state lock: {:?}", e);
                    return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
                }
            };

            let snapshot_sync_state_lock_height =
                snapshot_sync_state_lock_height.get_snapshot_height();
            if snapshot_sync_state_lock_height >= snapshot.height {
                warn!(
                        "Offered Snapshot height {:?} is less than or equal to the last locked snapshot height {:?}, rejecting snapshot",
                        snapshot.height, snapshot_sync_state_lock_height
                    );
                return ResponseOfferSnapshot { result: SnapshotOfferResult::Reject as i32 };
            }
        }

        match self.create_new_snapshot_sync(
            snapshot.height,
            B256::new(snapshot.hash.as_ref().try_into().expect("slice with incorrect length")),
            snapshot.chunks as u64,
            snapshot.format as u64,
        ) {
            Ok(_snapshot_id) => {
                // update the rw lock here as we now want to sync against that offered snapshot
                if let Some(snapshot_sync_state_lock) = self.snapshot_sync_state_lock.as_ref() {
                    let mut snapshot_sync_state_lock = match snapshot_sync_state_lock.write() {
                        Ok(snapshot_sync_state_lock) => snapshot_sync_state_lock,
                        Err(e) => {
                            error!("Error getting a snapshot state lock: {:?}", e);
                            return ResponseOfferSnapshot {
                                result: SnapshotOfferResult::Reject as i32,
                            };
                        }
                    };

                    (*snapshot_sync_state_lock)
                        .set_snapshot_height(snapshot.height)
                        .set_snapshot_hash(prost::bytes::Bytes::copy_from_slice(
                            snapshot.hash.as_ref(),
                        ))
                        .set_snapshot_chunks(snapshot.chunks as u64)
                        .set_snapshot_format(snapshot.format as u64);
                    drop(snapshot_sync_state_lock);
                };
                // we have accepted the snapshot already, just re-accept it

                ResponseOfferSnapshot { result: SnapshotOfferResult::Accept as i32 }
            }
            Err(e) => {
                error!("error persisting new snapshot sync: {:?}", e);

                ResponseOfferSnapshot { result: SnapshotOfferResult::Unknown as i32 }
            }
        }
    }

    /// https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#loadsnapshotchunk
    #[instrument(level = "trace", skip(self, request), fields(height = request.height, chunk = request.chunk))]
    fn load_snapshot_chunk(&self, request: RequestLoadSnapshotChunk) -> ResponseLoadSnapshotChunk {
        trace!("request={:?}", request);

        let snapshot_manager_state_lock = match self.snapshot_manager_state_lock.read() {
            Ok(snapshot_manager_state_lock) => snapshot_manager_state_lock,
            Err(e) => {
                error!("Error getting a snapshot state lock: {:?}", e);

                let response = ResponseLoadSnapshotChunk::default();

                trace!("return={:?}", response);

                return response;
            }
        };

        if snapshot_manager_state_lock.is_syncing_history() {
            drop(snapshot_manager_state_lock);
            debug!("Historical syncing ongoing. No snapshots available yet ...");

            let response = ResponseLoadSnapshotChunk::default();

            trace!("return={:?}", response);

            return response;
        }

        let client = self.storage.client.clone();

        // check if the snapshot is already applied
        let snapshot_manager_state_lock_block_id = snapshot_manager_state_lock.get_block_id();
        drop(snapshot_manager_state_lock);

        // check that we are not being asked to load the snapshot that we are currently syncing as
        // it might not be ready yet
        if snapshot_manager_state_lock_block_id == request.height {
            warn!("Received snapshot height matches current block height, rejecting snapshot as it might not be ready yet");

            let response = ResponseLoadSnapshotChunk::default();

            trace!("return={:?}", response);

            return response;
        }

        let response = match client.get_snapshot_id_by_block_id(request.height) {
            Ok(Some(snapshot_id)) => {
                // now take the entire snapshot data
                match client.get_snapshot_by_id(snapshot_id) {
                    Ok(Some(snapshot)) => {
                        // NOTE: all cometbft numeration starts at 0
                        let requested_chunk_index = request.chunk;
                        let chunk_id =
                            match snapshot.chunk_ids().get(requested_chunk_index as usize) {
                                Some(chunk_id) => *chunk_id,
                                None => {
                                    error!(
                                    "Requested chunk with index {:?} not found in snapshot {:?}",
                                    request.chunk, snapshot_id
                                );

                                    let response = ResponseLoadSnapshotChunk::default();

                                    trace!("return={:?}", response);

                                    return response;
                                }
                            };

                        match client.get_chunk_by_id(chunk_id) {
                            Ok(Some(chunk)) => {
                                let (oneshot_tx, oneshot_rx) = tokio::sync::oneshot::channel();
                                let compressor = self.compressor.clone();

                                self.task_executor.spawn_blocking(Box::pin(async move {
                                    let mut blocks: Vec<BlockWithSenders> = Vec::new();
                                    for chunk in chunk.chunk_data() {
                                        if let Ok(block_with_sender) =
                                            compressor.decode(chunk.as_ref()).await
                                        {
                                            blocks.push(block_with_sender);
                                        }
                                    }
                                    if let Ok(serialized_blocks) = compressor.encode(&blocks).await
                                    {
                                        let _ = oneshot_tx.send(serialized_blocks);
                                    }
                                }));

                                let serialized_blocks = match oneshot_rx.blocking_recv() {
                                    Ok(serialized_blocks) => serialized_blocks,
                                    Err(e) => {
                                        error!("Error on receiving serialized blocks from channel {:?}", e);

                                        let response = ResponseLoadSnapshotChunk::default();

                                        trace!("return={:?}", response);

                                        return response;
                                    }
                                };

                                let res = ResponseLoadSnapshotChunk {
                                    chunk: prost::bytes::Bytes::copy_from_slice(
                                        serialized_blocks.as_ref(),
                                    ),
                                };

                                res
                            }
                            Ok(None) => {
                                error!("Chunk with id {:?} not found", chunk_id);
                                ResponseLoadSnapshotChunk::default()
                            }
                            Err(e) => {
                                error!(
                                    "DB error getting chunk with id: {:?}. Error = {:?}",
                                    chunk_id, e
                                );
                                ResponseLoadSnapshotChunk::default()
                            }
                        }
                    }
                    Ok(None) => {
                        error!("Snapshot with id {:?} not found", snapshot_id);

                        ResponseLoadSnapshotChunk::default()
                    }
                    Err(e) => {
                        error!(
                            "DB error getting snapshot with id: {:?}. Error = {:?}",
                            snapshot_id, e
                        );
                        ResponseLoadSnapshotChunk::default()
                    }
                }
            }
            Ok(None) => {
                error!("Snapshot at height {:?} not found", request.height);
                ResponseLoadSnapshotChunk::default()
            }
            Err(e) => {
                error!(
                    "DB error getting snapshot at height: {:?}. Error = {:?}",
                    request.height, e
                );
                ResponseLoadSnapshotChunk::default()
            }
        };

        trace!("return={:?}", ResponseLoadSnapshotChunkTruncatedDebug(&response));

        response
    }

    /// https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#applysnapshotchunk
    #[instrument(level = "trace", ret, skip(self, request), fields(index = request.index))]
    fn apply_snapshot_chunk(
        &self,
        request: RequestApplySnapshotChunk,
    ) -> ResponseApplySnapshotChunk {
        trace!("request={:?}", RequestApplySnapshotChunkTruncatedDebug(&request));

        // ensure no historical sync is ongoing
        let snapshot_manager_state_lock = match self.snapshot_manager_state_lock.read() {
            Ok(snapshot_manager_state_lock) => snapshot_manager_state_lock,
            Err(e) => {
                error!("Error getting a snapshot state lock: {:?}", e);
                return ResponseApplySnapshotChunk {
                    result: ApplySnapshotResult::RetrySnapshot as i32,
                    refetch_chunks: vec![],
                    reject_senders: vec![],
                };
            }
        };

        if snapshot_manager_state_lock.is_syncing_history() {
            drop(snapshot_manager_state_lock);
            debug!("Historical syncing ongoing. No snapshots available yet ...");
            return ResponseApplySnapshotChunk {
                result: ApplySnapshotResult::RetrySnapshot as i32,
                refetch_chunks: vec![],
                reject_senders: vec![],
            };
        }

        let client = self.storage.client.clone();

        // get the last snapshot sync id - there should always be one provided the offer_snapshot
        // has already run
        let last_snapshot_sync_id = match client.get_last_snapshot_sync_id() {
            Ok(Some(snapshot_sync_id)) => snapshot_sync_id,
            Ok(None) => {
                error!("No last snapshot sync found");
                return ResponseApplySnapshotChunk {
                    result: ApplySnapshotResult::RetrySnapshot as i32,
                    refetch_chunks: vec![],
                    reject_senders: vec![],
                };
            }
            Err(e) => {
                error!("Error getting last snapshot sync: {:?}", e);
                return ResponseApplySnapshotChunk {
                    result: ApplySnapshotResult::RetrySnapshot as i32,
                    refetch_chunks: vec![],
                    reject_senders: vec![],
                };
            }
        };

        // fetch the actual snapshot sync
        let mut snapshot = match client.get_snapshot_sync_by_id(last_snapshot_sync_id) {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => {
                error!("No snapshot sync found by id");
                return ResponseApplySnapshotChunk {
                    result: ApplySnapshotResult::RetrySnapshot as i32,
                    refetch_chunks: vec![],
                    reject_senders: vec![],
                };
            }
            Err(e) => {
                error!("Error getting snapshot sync by id: {:?}", e);
                return ResponseApplySnapshotChunk {
                    result: ApplySnapshotResult::RetrySnapshot as i32,
                    refetch_chunks: vec![],
                    reject_senders: vec![],
                };
            }
        };

        // check the snapshot sync is done in sequential manner
        info!("last applied chunk index: {:?}", snapshot.last_applied_chunk_index());

        // request index will be ahead `last_applied_chunk_index` by 1 except for the first chunk
        if snapshot.last_applied_chunk_index().saturating_sub(1) > request.index as u64 {
            error!("Last applied chunk index is not sequential with the incoming chunk index");
            return ResponseApplySnapshotChunk {
                result: ApplySnapshotResult::RetrySnapshot as i32,
                refetch_chunks: vec![],
                reject_senders: vec![],
            };
        }

        // set the last applied chunk index
        snapshot.set_last_applied_chunk_index(request.index as u64);

        // update the db
        if let Err(e) = self.update_snapshot_sync(last_snapshot_sync_id, snapshot.clone()) {
            error!(
                "Error updating snapshot sync {:?} in the db. error = {:?}",
                last_snapshot_sync_id, e
            );
            return ResponseApplySnapshotChunk {
                result: ApplySnapshotResult::RetrySnapshot as i32,
                refetch_chunks: vec![],
                reject_senders: vec![],
            };
        }

        // decompress and decode the snapshot chunk (= n blocks) and apply it
        let compressor = self.compressor.clone();
        let (compressor_task_tx, compressor_task_rx) =
            tokio::sync::oneshot::channel::<Vec<BlockWithSenders>>();
        self.task_executor.spawn_blocking(Box::pin(async move {
            match compressor.decode(request.chunk.as_ref()).await {
                Ok(blocks_with_senders) => {
                    let _ = compressor_task_tx.send(blocks_with_senders);
                }
                Err(e) => {
                    error!("Failed to deserialize and decompress snapshot chunk: {:?}", e);
                }
            };
        }));

        // await the response from the compressor task
        let blocks_with_senders = match compressor_task_rx.blocking_recv() {
            Ok(blocks_with_senders) => blocks_with_senders,
            Err(e) => {
                error!("Failed to receive blocks from compressor task: {:?}", e);
                return ResponseApplySnapshotChunk {
                    result: ApplySnapshotResult::RetrySnapshot as i32,
                    refetch_chunks: vec![],
                    reject_senders: vec![],
                };
            }
        };

        let exec_outcome = match batch_execute(
            blocks_with_senders.clone(),
            &self.provider_factory,
            self.storage.executor_factory.clone(),
        ) {
            Ok(exec_outcome) => exec_outcome,
            Err(e) => {
                error!("Error executing blocks: {:?}", e);
                return ResponseApplySnapshotChunk {
                    result: ApplySnapshotResult::RetrySnapshot as i32,
                    refetch_chunks: vec![],
                    reject_senders: vec![],
                };
            }
        };

        let provider = match self.provider_factory.provider_rw() {
            Ok(provider) => provider,
            Err(e) => {
                error!("Error getting provider: {:?}", e);
                return ResponseApplySnapshotChunk { ..Default::default() };
            }
        };

        let hashed_state = exec_outcome.hash_state_slow();
        let (_state_root, trie_updates) =
            match StateRoot::overlay_root_with_updates(provider.tx_ref(), hashed_state.clone()) {
                Ok((state_root, trie_updates)) => (state_root, trie_updates),
                Err(e) => {
                    error!("Error overlaying root with updates: {:?}", e);
                    return ResponseApplySnapshotChunk { ..Default::default() };
                }
            };

        // seal blocks
        let sealed_blocks_with_senders =
            blocks_with_senders.into_iter().map(|block| block.seal_slow()).collect::<Vec<_>>();

        if let Err(e) = provider.append_blocks_with_state(
            sealed_blocks_with_senders.clone(),
            exec_outcome.clone(),
            hashed_state.into_sorted(),
            trie_updates.clone(),
        ) {
            error!(
                "Error appending blocks with state {:?} in the db. error = {:?}",
                last_snapshot_sync_id, e
            );
            return ResponseApplySnapshotChunk {
                result: ApplySnapshotResult::RetrySnapshot as i32,
                refetch_chunks: vec![],
                reject_senders: vec![],
            };
        }

        if let Err(e) = provider.commit() {
            error!(
                "Error committing db after appending blocks with state {:?} in the db. error = {:?}",
                last_snapshot_sync_id, e
            );
            return ResponseApplySnapshotChunk {
                result: ApplySnapshotResult::RetrySnapshot as i32,
                refetch_chunks: vec![],
                reject_senders: vec![],
            };
        };

        for sealed_block_with_senders in sealed_blocks_with_senders.into_iter() {
            let senders = sealed_block_with_senders.senders().unwrap_or_default();
            let hashed_state = exec_outcome.hash_state_slow();
            let block_height = sealed_block_with_senders.block.number;
            let new_header = sealed_block_with_senders.block.header.clone();
            let sealed_block = sealed_block_with_senders.block.clone();

            let executed_block = ExecutedBlock::new(
                Arc::new(sealed_block),
                Arc::new(senders),
                Arc::new(exec_outcome.clone()),
                Arc::new(hashed_state.clone()),
                Arc::new(trie_updates.clone()),
            );

            let new_chain =
                reth_chain_state::NewCanonicalChain::Commit { new: vec![executed_block] };
            self.blockchain_db.canonical_in_memory_state().update_chain(new_chain);

            self.blockchain_db.on_forkchoice_update_received(&ForkchoiceState::default());
            self.blockchain_db.set_canonical_head(new_header.clone());
            self.blockchain_db.set_safe(new_header.clone());
            self.blockchain_db.set_finalized(new_header.clone());

            self.blockchain_db
                .canonical_in_memory_state()
                .remove_persisted_blocks(block_height - 1);

            let chain =
                Chain::new(vec![sealed_block_with_senders].into_iter(), exec_outcome.clone(), None);

            // Note: we are not parsing the block for pegins and pegouts here.
            // This is safe for rpc nodes but not for the federation nodes especially the
            // coordinator: If the coordinator uses snapshots, it will be unaware of
            // pending pegouts that need to be honored. The coordinator creates pegouts
            // from pending pegouts in it's database. The coordinator must use block
            // sync instead of snapshot sync. TODO: refactor to handle pegins/pegouts
            self.blockchain_db.canonical_in_memory_state().notify_canon_state(
                CanonStateNotification::Commit {
                    new: Arc::new(chain),
                    pegins: None,
                    pegouts: None,
                },
            );
        }

        ResponseApplySnapshotChunk {
            result: ApplySnapshotResult::Accept as i32,
            refetch_chunks: vec![],
            reject_senders: vec![],
        }
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#prepareProposal
    #[instrument(level = "trace", skip(self, request), fields(cbft_block_height = request.height))]
    fn prepare_proposal(&self, request: RequestPrepareProposal) -> ResponsePrepareProposal {
        let execution_start_time = std::time::Instant::now();
        trace!("request={:?}", request);

        if !request.txs.is_empty() {
            panic!(
                "Transactions are not expected from CometBFT mempool to propose on height {}",
                request.height
            );
        }

        let block_time = request.time.expect("block time is not set in the request");

        let max_tx_bytes: usize =
            request.max_tx_bytes.try_into().expect("Invalid request proposal max_tx_bytes value");

        // create non-deterministic data tx at index 0 so historical sync will pass verification
        let non_deterministic_data = match self.non_deterministic_data() {
            Ok(ndd) => ndd,
            Err(e) => {
                panic!(
                    "Error creating non-deterministic data for proposal on height {}: {:?}",
                    request.height, e
                );
            }
        };

        trace!("non_deterministic_data={:?}", non_deterministic_data);

        // serialize non-deterministic data tx to bytes
        let non_deterministic_data_bytes = match self
            .serialize_non_deterministic_data_to_bytes(non_deterministic_data)
        {
            Ok(bytes) => bytes,
            Err(e) => {
                panic!("Error serializing non-deterministic data bytes for proposal on height {}: {:?}", request.height, e);
            }
        };

        // NDD goes to a block as the first transaction
        // so we need to take into account its size

        let non_deterministic_data_bytes_len = non_deterministic_data_bytes.len();
        if non_deterministic_data_bytes_len > max_tx_bytes {
            // We should panic bc there is a critical bug and there should be a chain halt.
            panic!(
                "Non-deterministic data size to propose for height {}: {} exceeds the max tx bytes allowed size {}",
                request.height, non_deterministic_data_bytes_len, max_tx_bytes
            );
        };

        let max_tx_bytes = max_tx_bytes - non_deterministic_data_bytes_len;

        // Nothing to process if mempool is empty
        // propose an empty block with NDD only
        if self.pool.pool_size().total == 0 {
            debug!("No transactions in pool, proposing empty cbft block with NDD only");

            let response = ResponsePrepareProposal { txs: vec![non_deterministic_data_bytes] };

            trace!("return={:?}", response);

            if tracing::enabled!(tracing::Level::INFO) {
                let execution_time = execution_start_time.elapsed().as_secs_f32();

                info!(
                    block_time = block_time.seconds,
                    cbft_transactions_count = 1,
                    eth_transactions_count = 0,
                    execution_time,
                    "Prepared a proposal with 1 transaction in {} seconds",
                    execution_time,
                );
            }

            return response;
        }

        let payload_config = match self.payload_builder_arguments() {
            Ok(payload_config) => payload_config,
            Err(e) => {
                panic!(
                    "error building payload config for proposal on height {}: {:?}",
                    request.height, e
                );
            }
        };
        let client = self.storage.client.clone();

        let build_args = BuildArguments {
            client,
            pool: self.pool.clone(),
            cached_reads: Default::default(),
            config: payload_config,
            cancel: Default::default(),
            best_payload: None,
            max_tx_bytes: Some(max_tx_bytes),
        };

        match default_ethereum_payload_builder(self.storage.evm_config, build_args) {
            Ok(res) => {
                match res {
                    reth_basic_payload_builder::BuildOutcome::Aborted { fees, cached_reads: _ } => {
                        // TODO: Aborted why, shall we just propose NDD?
                        panic!(
                            "aborted payload building because resulted in worse block wrt. fees {} for height {}", fees, request.height
                        );
                    }
                    reth_basic_payload_builder::BuildOutcome::Cancelled => {
                        // TODO: Canceled why, shall we just propose NDD?
                        panic!(
                            "aborted payload building because cancelled for height {}",
                            request.height
                        );
                    }
                    reth_basic_payload_builder::BuildOutcome::Better {
                        payload,
                        cached_reads: _,
                    } => {
                        let block = payload.block();

                        trace!("eth_block_header={:?}", block.header);

                        // These are bytes of [SignedTransaction]
                        let mut txs: Vec<_> = block
                            .raw_transactions()
                            .iter()
                            .map(|tx| prost::bytes::Bytes::copy_from_slice(tx))
                            .collect::<_>();

                        // insert non-deterministic data tx at index 0 so historical sync will pass
                        // verification

                        txs.insert(0, non_deterministic_data_bytes);

                        self.metrics.commet_prepared_proposals.increment(1);

                        let txs_len = txs.len();

                        let response = ResponsePrepareProposal { txs };

                        trace!("return={:?}", ResponsePrepareProposalTruncatedDebug(&response));

                        if tracing::enabled!(tracing::Level::INFO) {
                            let execution_time = execution_start_time.elapsed().as_secs_f32();

                            info!(
                                block_time = block_time.seconds,
                                execution_time,
                                cbft_transactions_count = txs_len,
                                eth_transactions_count = txs_len - 1, // Minus NDD
                                "Prepared a proposal with {} transactions in {} seconds",
                                txs_len,
                                execution_time,
                            );
                        }

                        response
                    }
                }
            }
            Err(e) => {
                panic!("error building payload for proposal on height {}: {:?}", request.height, e);
            }
        }
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#checktx
    fn check_tx(&self, _request: RequestCheckTx) -> ResponseCheckTx {
        error!("check_tx method is called. CometBFT mempool is not supported.");
        ResponseCheckTx {
            code: 1,
            log: "CometBFT mempool is not supported".to_string(),
            ..Default::default()
        }
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#prepareproposal
    #[instrument(level = "trace", ret, skip(self, request), fields(cfbt_block_height = request.height, cbft_block_hash = hex::encode(&request.hash)))]
    fn process_proposal(&self, request: RequestProcessProposal) -> ResponseProcessProposal {
        let execution_start_time = std::time::Instant::now();
        trace!("request={:?}", RequestProcessProposalTruncatedDebug(&request));

        let txs_len = request.txs.len();

        let agg_pk = match self.aggregate_public_key() {
            Ok(pk) => pk,
            Err(_) => {
                // Fed nodes must always have an aggregate public key
                if self.is_fed_node {
                    warn!("Aggregate public key for fed node is not set in process proposal");
                }

                // Rpc nodes will have an aggregate public key above block height 1
                if request.height > 1 {
                    warn!("Aggregate public key for rpc node is not set in process proposal");
                }

                if tracing::enabled!(tracing::Level::WARN) {
                    let execution_time = execution_start_time.elapsed().as_secs_f32();
                    let app_hash = match self.application_hash(&self.storage.client) {
                        Ok(app_hash) => app_hash,
                        Err(e) => {
                            panic!("failed to get application hash on process proposal: {:?}", e);
                        }
                    };

                    warn!(
                        app_hash = hex::encode(&app_hash),
                        execution_time,
                        "A proposal with {} transactions is rejected in {} seconds",
                        request.txs.len(),
                        execution_time
                    );
                }

                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        };

        // Extract block time: this must come from the CBFT block header NOT the system time
        // As that will be underministic
        let block_time = match request.time {
            Some(time) => time,
            None => {
                error!("Block time is not set in process proposal");
                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        };

        let cbft_block_hash = FixedBytes::<32>::from_slice(request.hash.to_vec().as_slice());

        // extract first tx which contains non-deterministic data and validate
        let txs_bytes = request.txs;
        let non_deterministic_data_bytes = match txs_bytes.first() {
            Some(tx) => tx.clone(),
            None => {
                warn!("No non-deterministic data in proposal request");

                if tracing::enabled!(tracing::Level::WARN) {
                    let execution_time = execution_start_time.elapsed().as_secs_f32();
                    let app_hash = match self.application_hash(&self.storage.client) {
                        Ok(app_hash) => app_hash,
                        Err(e) => {
                            panic!("failed to get application hash on process proposal: {:?}", e);
                        }
                    };

                    warn!(
                        app_hash = hex::encode(&app_hash),
                        block_time = block_time.seconds,
                        execution_time,
                        cbft_transactions_count = txs_len,
                        eth_transactions_count = txs_len - 1,
                        "A proposal with {} transactions is rejected in {} seconds",
                        txs_len,
                        execution_time
                    );
                }

                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        };

        let reader_inner: Vec<u8> =
            vec![non_deterministic_data_bytes].into_iter().flatten().collect();
        let reader = &mut io::Cursor::new(reader_inner);

        let non_deterministic_data = match NonDeterministicData::deserialize(reader) {
            Ok(data) => data,
            Err(e) => {
                trace!(
                    "non_deterministic_data_bytes={:?}",
                    hex::encode(txs_bytes.first().expect("txs_bytes contains first transaction"))
                );

                warn!("Error deserializing non-deterministic data: {:?}", e);

                if tracing::enabled!(tracing::Level::WARN) {
                    let execution_time = execution_start_time.elapsed().as_secs_f32();
                    let app_hash = match self.application_hash(&self.storage.client) {
                        Ok(app_hash) => app_hash,
                        Err(e) => {
                            panic!("failed to get application hash on process proposal: {:?}", e);
                        }
                    };

                    warn!(
                        app_hash = hex::encode(&app_hash),
                        block_time = block_time.seconds,
                        execution_time,
                        cbft_transactions_count = txs_len,
                        eth_transactions_count = txs_len - 1,
                        "A proposal with {} transactions is rejected in {} seconds",
                        txs_len,
                        execution_time
                    );
                }

                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        };

        if non_deterministic_data.version() != LATEST_NDD_VERSION {
            warn!(
                ?non_deterministic_data,
                "processing block with unknown non-deterministic data version"
            );
        }

        trace!("non_deterministic_data={:?}", non_deterministic_data);

        // check non-deterministic data: btc block hash and aggregate public key
        if !self.bitcoin_checkpoints.contains_by_hash(non_deterministic_data.bitcoin_block_hash) {
            warn!(
                checkpoints_chain = %self.bitcoin_checkpoints,
                proposed_checkpoint_hash = %non_deterministic_data.bitcoin_block_hash,
                "A proposed bitcoin checkpoint is not a part of local checkpoint chain. Most probably a proposer's or local bitcoin node is out of sync."
            );

            if tracing::enabled!(tracing::Level::WARN) {
                let execution_time = execution_start_time.elapsed().as_secs_f32();
                let app_hash = match self.application_hash(&self.storage.client) {
                    Ok(app_hash) => app_hash,
                    Err(e) => {
                        panic!("failed to get application hash on process proposal: {:?}", e);
                    }
                };

                warn!(
                    app_hash = hex::encode(&app_hash),
                    block_time = block_time.seconds,
                    execution_time,
                    cbft_transactions_count = txs_len,
                    eth_transactions_count = txs_len - 1,
                    "A proposal with {} transactions is rejected in {} seconds",
                    txs_len,
                    execution_time
                );
            }

            return ResponseProcessProposal { status: VERIFY_REJECT };
        }

        if agg_pk != non_deterministic_data.aggregated_public_key {
            warn!("Aggregate public key mismatch");

            if tracing::enabled!(tracing::Level::WARN) {
                let execution_time = execution_start_time.elapsed().as_secs_f32();
                let app_hash = match self.application_hash(&self.storage.client) {
                    Ok(app_hash) => app_hash,
                    Err(e) => {
                        panic!("failed to get application hash on process proposal: {:?}", e);
                    }
                };

                warn!(
                    app_hash = hex::encode(&app_hash),
                    block_time = block_time.seconds,
                    execution_time,
                    cbft_transactions_count = txs_len,
                    eth_transactions_count = txs_len - 1,
                    "A proposal with {} transactions is rejected in {} seconds",
                    txs_len,
                    execution_time
                );
            }

            return ResponseProcessProposal { status: VERIFY_REJECT };
        }

        // get txs skipping the first non-deterministic data tx
        let txs = match transactions_signed_from_bytes(txs_bytes.iter().skip(1).cloned()) {
            Ok(txs) => txs,
            Err(e) => {
                error!("Error decoding transactions: {:?}", e);

                if tracing::enabled!(tracing::Level::WARN) {
                    let execution_time = execution_start_time.elapsed().as_secs_f32();
                    let app_hash = match self.application_hash(&self.storage.client) {
                        Ok(app_hash) => app_hash,
                        Err(e) => {
                            panic!("failed to get application hash on process proposal: {:?}", e);
                        }
                    };

                    warn!(
                        app_hash = hex::encode(&app_hash),
                        block_time = block_time.seconds,
                        execution_time,
                        cbft_transactions_count = txs_len,
                        eth_transactions_count = txs_len - 1,
                        "A proposal with {} transactions is rejected in {} seconds",
                        txs_len,
                        execution_time
                    );
                }

                return ResponseProcessProposal { status: VERIFY_REJECT };
            }
        };

        // Validation done as a result of this call:
        // - botanix consensus package created on the fly and compared to the incoming block EDH
        // - mint validation checks
        // - state trie calculated for header
        // This means no additional validation is needed when the ABCI driver inserts the block into
        // the canonical chain
        match build_and_execute(
            txs,
            self.storage.chain_spec.clone(),
            &non_deterministic_data.block_fee_recipient_address,
            self.storage.evm_config,
            &self.provider_factory,
            &self.storage.bitcoind_factory,
            self.storage.btc_network,
            &non_deterministic_data.bitcoin_block_hash,
            &agg_pk,
            block_time,
        ) {
            Ok(block_with_context) => {
                let block = block_with_context.sealed_block_with_peg.block();

                // validate block before caching
                if !matches!(
                    self.validate_block(block),
                    ResponseProcessProposal { status: VERIFY_ACCEPTED }
                ) {
                    // we have logs inside validate_block so no need to repeat error here

                    if tracing::enabled!(tracing::Level::WARN) {
                        let execution_time = execution_start_time.elapsed().as_secs_f32();
                        let app_hash = match self.application_hash(&self.storage.client) {
                            Ok(app_hash) => app_hash,
                            Err(e) => {
                                panic!(
                                    "failed to get application hash on process proposal: {:?}",
                                    e
                                );
                            }
                        };

                        warn!(
                            app_hash = hex::encode(&app_hash),
                            block_time = block_time.seconds,
                            execution_time,
                            cbft_transactions_count = txs_len,
                            eth_transactions_count = txs_len - 1,
                            "A proposal with {} transactions is rejected in {} seconds",
                            txs_len,
                            execution_time
                        );
                    }

                    return ResponseProcessProposal { status: VERIFY_REJECT };
                }

                match self.block_cache.write() {
                    Ok(mut block_cache_write) => {
                        let eth_block_hash = block.hash();

                        debug!(
                            cbft_block_hash = hex::encode(cbft_block_hash),
                            eth_block_hash = hex::encode(eth_block_hash),
                            "update eth block cache",
                        );

                        block_cache_write.cache.insert(cbft_block_hash, block_with_context);

                        self.metrics.commet_processed_proposals.increment(1);

                        if tracing::enabled!(tracing::Level::INFO) {
                            let execution_time = execution_start_time.elapsed().as_secs_f32();

                            info!(
                                app_hash = hex::encode(eth_block_hash),
                                block_time = block_time.seconds,
                                execution_time,
                                cbft_transactions_count = txs_len,
                                eth_transactions_count = txs_len - 1, // Minus NDD
                                "Processed a proposal with {} transactions in {} seconds",
                                txs_len,
                                execution_time,
                            );
                        }

                        ResponseProcessProposal { status: VERIFY_ACCEPTED }
                    }
                    Err(e) => {
                        error!("Error getting block cache write lock: {:?}", e);

                        if tracing::enabled!(tracing::Level::WARN) {
                            let execution_time = execution_start_time.elapsed().as_secs_f32();
                            let app_hash = match self.application_hash(&self.storage.client) {
                                Ok(app_hash) => app_hash,
                                Err(e) => {
                                    panic!(
                                        "failed to get application hash on process proposal: {:?}",
                                        e
                                    );
                                }
                            };

                            warn!(
                                app_hash = hex::encode(&app_hash),
                                block_time = block_time.seconds,
                                execution_time,
                                cbft_transactions_count = txs_len,
                                eth_transactions_count = txs_len - 1,
                                "A proposal with {} transactions is rejected in {} seconds",
                                txs_len,
                                execution_time
                            );
                        }

                        ResponseProcessProposal { status: VERIFY_REJECT }
                    }
                }
            }
            Err(e) => {
                error!("Error building eth block: {:?}", e);

                if tracing::enabled!(tracing::Level::WARN) {
                    let execution_time = execution_start_time.elapsed().as_secs_f32();
                    let app_hash = match self.application_hash(&self.storage.client) {
                        Ok(app_hash) => app_hash,
                        Err(e) => {
                            panic!("failed to get application hash on process proposal: {:?}", e);
                        }
                    };

                    warn!(
                        app_hash = hex::encode(&app_hash),
                        block_time = block_time.seconds,
                        execution_time,
                        cbft_transactions_count = txs_len,
                        eth_transactions_count = txs_len - 1,
                        "A proposal with {} transactions is rejected in {} seconds",
                        txs_len,
                        execution_time
                    );
                }

                ResponseProcessProposal { status: VERIFY_REJECT }
            }
        }
    }

    ///docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#finalizeblock
    #[instrument(level = "trace", skip(self, request), fields(cbft_block_height = request.height, cbft_block_hash = hex::encode(&request.hash)))]
    fn finalize_block(&self, request: RequestFinalizeBlock) -> ResponseFinalizeBlock {
        trace!("request={:?}", RequestFinalizeBlockTruncatedDebug(&request));

        if request.txs.is_empty() {
            panic!("No transactions in finalize_block request, but expected at least NDD tx");
        }

        let cbft_block_hash = FixedBytes::<32>::from_slice(request.hash.to_vec().as_slice());
        let mut block_cache_write = match self.block_cache.write() {
            Ok(block_cache_write) => block_cache_write,
            Err(e) => {
                panic!("Error getting eth block cache write lock: {:?}", e);
            }
        };

        let block_with_context = match block_cache_write.cache.get(&cbft_block_hash) {
            Some(block_with_context) => {
                debug!(
                    cbft_block_hash = hex::encode(cbft_block_hash),
                    eth_block_hash =
                        hex::encode(block_with_context.sealed_block_with_peg.block().hash()),
                    "read eth block from block cache",
                );

                block_with_context.clone()
            }
            None => {
                // No block in cache: this happens during historical (block) sync
                // Build the block

                debug!(
                    cbft_block_hash = hex::encode(cbft_block_hash),
                    "eth block not found in block cache, building a block"
                );

                // get non-deterministic data
                let txs_bytes = request.txs.clone();
                let non_deterministic_data_bytes = match txs_bytes.clone().first() {
                    Some(tx) => tx.clone(),
                    None => {
                        panic!("No non-deterministic tx in finalize block request");
                    }
                };
                let reader_inner: Vec<u8> =
                    vec![non_deterministic_data_bytes].into_iter().flatten().collect();
                let reader = &mut io::Cursor::new(reader_inner);

                let non_deterministic_data = match NonDeterministicData::deserialize(reader) {
                    Ok(data) => data,
                    Err(e) => {
                        panic!("Error deserializing non-deterministic data: {:?}", e);
                    }
                };

                let block_time = match request.time {
                    Some(time) => time,
                    None => {
                        panic!("Block time is not set in process proposal");
                    }
                };

                // get txs skipping the first non-deterministic data tx
                let txs_iter = txs_bytes.clone().into_iter().skip(1);
                let txs = if txs_iter.clone().next().is_none() {
                    vec![]
                } else {
                    match transactions_signed_from_bytes(txs_iter) {
                        Ok(txs) => txs,
                        Err(e) => {
                            panic!("Error decoding transactions in finalize block: {:?}", e);
                        }
                    }
                };

                match build_and_execute(
                    txs,
                    self.storage.chain_spec.clone(),
                    &non_deterministic_data.block_fee_recipient_address,
                    self.storage.evm_config,
                    &self.provider_factory,
                    &self.storage.bitcoind_factory,
                    self.storage.btc_network,
                    &non_deterministic_data.bitcoin_block_hash,
                    &non_deterministic_data.aggregated_public_key,
                    block_time,
                ) {
                    Ok(block_with_context) => {
                        block_cache_write.cache.insert(cbft_block_hash, block_with_context.clone());

                        debug!(
                            cbft_block_hash = hex::encode(cbft_block_hash),
                            eth_block_hash = hex::encode(
                                block_with_context.sealed_block_with_peg.block().hash()
                            ),
                            "update eth block cache",
                        );

                        block_with_context
                    }
                    Err(e) => {
                        panic!("Error building block in finalize block: {:?}", e);
                    }
                }
            }
        };

        // Track the finalized block hash for the commit stage.
        block_cache_write.tracked_final = Some(cbft_block_hash);

        // Rpc node needs to store aggregate public key from block height 1
        let block_height = block_with_context.sealed_block_with_peg.block().number;
        let sealed_block_with_peg_binding = block_with_context.sealed_block_with_peg.clone();
        let sealed_block_with_senders = sealed_block_with_peg_binding.block();
        // TODO: Shouldn't it be done on block commit?
        if !self.is_fed_node && block_height == 1 {
            let edh = match sealed_block_with_senders.deserialize_extra_data_header() {
                Ok(edh) => edh,
                Err(e) => {
                    panic!("Error deserializing extra data header in finalize block: {:?}", e);
                }
            };

            let mut storage = self.storage.inner.blocking_write();
            storage.aggregate_public_key = Some(edh.aggregated_public_key);
        }

        if matches!(
            self.validate_block(block_with_context.sealed_block_with_peg.block()),
            ResponseProcessProposal { status: VERIFY_REJECT }
        ) {
            panic!("failed to finalize invalid block block {:?}", request.height);
        }

        let mut exec_results = vec![];
        // insert non-deterministic data tx which is first in the block (already checked above)
        let non_deterministic_data_tx = match request.txs.first() {
            Some(tx) => tx.clone(),
            None => {
                panic!("failed to finalize block {} without NDD", request.height);
            }
        };

        let first_exec_tx_result =
            ExecTxResult { code: SUCCESS, data: non_deterministic_data_tx, ..Default::default() };
        exec_results.push(first_exec_tx_result);

        for _tx in block_with_context.sealed_block_with_peg.block().body.iter() {
            // https://docs.cometbft.com/v0.38/spec/abci/abci++_app_requirements#transaction-results
            exec_results.push(ExecTxResult {
                code: SUCCESS,
                // From https://docs.cometbft.com/v0.38/spec/abci/abci++_app_requirements#gas
                // In v0.34.x and earlier versions, CometBFT does not enforce anything about Gas in
                // consensus, only in the mempool. ... The GasUsed field is ignored
                // by CometBFT. CometBFT's genesis.json should have max_gas set to
                // -1 as to not enforce any gas limit restrictions Gas and other
                // block resource limits are enforced by the EVM/Reth
                ..Default::default()
            });
        }

        let block_hash = block_with_context.sealed_block_with_peg.block().hash();
        self.metrics.commet_finalized_blocks.increment(1);

        let execution_time = std::time::Instant::now().elapsed().as_secs_f32();

        let eth_block_hash_hex = hex::encode(block_hash);

        let txs_len = request.txs.len();

        info!(
            app_hash = eth_block_hash_hex,
            eth_block_hash = eth_block_hash_hex,
            cbft_block_hash = hex::encode(cbft_block_hash),
            cbft_transactions_count = txs_len,
            eth_transactions_count = txs_len - 1, // Minus NDD
            execution_time,
            "Finalized cbft block with {} transactions in {} seconds",
            txs_len,
            execution_time,
        );

        ResponseFinalizeBlock {
            events: vec![],
            tx_results: exec_results,
            validator_updates: vec![],
            consensus_param_updates: None,
            app_hash: prost::bytes::Bytes::copy_from_slice(&block_hash.0),
        }
    }

    /// docs: https://docs.cometbft.com/v0.38/spec/abci/abci++_methods#commit
    /// Panic! if there's an error bc that means the block hasn't
    /// been successfully committed to the database. There is no way to recover from
    /// an application hash mismatch other than a manual rollback of the db to a healthy state.
    /// Proceeding after an error will cause the app hash mismatch.
    #[instrument(level = "trace", skip(self), ret)]
    fn commit(&self) -> ResponseCommit {
        let execution_start_time = std::time::Instant::now();

        let Ok(mut block_cache_write) = self.block_cache.write() else {
            panic!("Error getting block cache write lock");
        };

        // Retrieve the finalized block via hash.
        let cbft_block_hash =
            block_cache_write.tracked_final.take().expect("No tracked final block hash");

        let Some(sealed_block_with_context) = block_cache_write.cache.remove(&cbft_block_hash)
        else {
            panic!("Error getting block from cache");
        };

        let block = sealed_block_with_context.sealed_block_with_peg.block();

        let cbft_block_hash_hex = hex::encode(cbft_block_hash);
        let eth_block_hash_hex = hex::encode(block.hash());

        debug!(
            cbft_block_hash = cbft_block_hash_hex,
            eth_block_hash = eth_block_hash_hex,
            "read finalized eth block from cache",
        );

        trace!("eth_block_header={:?}", block.header());

        let eth_block_height = sealed_block_with_context.sealed_block_with_peg.block().number;

        // We want to explicitly panic if we cannot send the commit message
        let (commit_tx, commit_rx) = std::sync::mpsc::channel::<()>();
        let driver_tx = self.driver_tx.clone();
        self.task_executor.spawn_blocking(Box::pin(async move {
            if let Err(e) = driver_tx
                .send(ABCIDriverMessage::CommitBlock((sealed_block_with_context, commit_tx)))
                .await
            {
                panic!("Error sending commit eth block message: {:?}", e);
            }
        }));
        if let Err(e) = commit_rx.recv() {
            panic!("Error receiving commit eth block response {e:?}");
        }

        let execution_time = execution_start_time.elapsed().as_secs_f32();

        info!(
            eth_block_height,
            app_hash = eth_block_hash_hex,
            cbft_block_hash = cbft_block_hash_hex,
            eth_block_hash = eth_block_hash_hex,
            execution_time,
            "The cbft block {} is committed in {} seconds",
            cbft_block_hash_hex,
            execution_time,
        );

        self.metrics.commet_committed_blocks.increment(1);

        ResponseCommit::default()
    }
}

/// ABCI driver message
#[derive(Debug)]
pub enum ABCIDriverMessage {
    /// Finalize a block, message includes the sealed block and the CBFT block hash
    CommitBlock((BlockWithContext, std::sync::mpsc::Sender<()>)),
    /// Exit the driver
    Exit,
}

/// The driver is mainly responsible for driving block completion and finalization
/// Once a finalize block is received the drive is responsible for
/// * Updating the canonical chain via DB
/// * Sending pegins / pegouts to the btc server

#[derive(Clone)]
pub struct ABCIDriver<DatabaseRW> {
    driver_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<ABCIDriverMessage>>>,
    database_provider: ProviderFactory<DatabaseRW, ChainSpec>,
    blockchain_provider: BlockchainProvider2<DatabaseRW>,
}

impl<DatabaseRW> ABCIDriver<DatabaseRW>
where
    DatabaseRW: Database + Clone + Send + Sync + 'static,
{
    /// Create a new ABCI drivers
    pub fn new(
        driver_rx: tokio::sync::mpsc::Receiver<ABCIDriverMessage>,
        database_provider: ProviderFactory<DatabaseRW, ChainSpec>,
        blockchain_provider: BlockchainProvider2<DatabaseRW>,
    ) -> Self {
        Self { driver_rx: Arc::new(Mutex::new(driver_rx)), database_provider, blockchain_provider }
    }

    /// Start the ABCI driver
    pub async fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        loop {
            if let Some(message) = self.driver_rx.lock().await.recv().await {
                match message {
                    ABCIDriverMessage::CommitBlock((sealed_block_with_context, commit_tx)) => {
                        let _span = tracing::trace_span!(
                            "ABCI driver commit block",
                            eth_block_height =
                                sealed_block_with_context.sealed_block_with_peg.block().number,
                            eth_block_hash =
                                %sealed_block_with_context.sealed_block_with_peg.block().hash(),
                        )
                        .entered();

                        let sealed_block_with_peg = sealed_block_with_context.sealed_block_with_peg;
                        let new_header = sealed_block_with_peg.block().header.clone();
                        let block_height = sealed_block_with_peg.block().number;
                        let sealed_block_with_senders = sealed_block_with_peg.block().to_owned();
                        let hashed_state = sealed_block_with_context.exec_outcome.hash_state_slow();
                        let trie_updates = sealed_block_with_context.trie_updates;

                        let executed_block = ExecutedBlock::new(
                            Arc::new(sealed_block_with_senders.block.clone()),
                            Arc::new(sealed_block_with_senders.senders.clone()),
                            Arc::new(sealed_block_with_context.exec_outcome.clone()),
                            Arc::new(hashed_state.clone()),
                            Arc::new(trie_updates.clone()),
                        );

                        let pegins = sealed_block_with_peg
                            .pegins()
                            .iter()
                            .flat_map(|p| p.meta.clone())
                            .collect::<Vec<_>>();

                        let pegouts = sealed_block_with_peg.pegouts().to_vec();

                        // Prepare the staged entries for insertion into the
                        // database; this ensures that no pegins or pegouts are
                        // ever accidentally dropped during a shutdown or
                        // interruption.
                        //
                        // Those staged entries are removed from the database
                        // once the Frost task has successfully initiated a new
                        // checkpoint on the btc-server.

                        let staged_pegins: Vec<models::PeginData> =
                            get_staged_pegins_from_pegin_meta(&pegins);
                        let staged_pegouts: Vec<models::PegoutData> =
                            get_staged_pegouts_from_pegout_data(&pegouts, new_header.number);

                        let header_with_pegs = models::HeaderWithPegs {
                            pegins: staged_pegins,
                            pegouts: staged_pegouts,
                            header: new_header.header().clone(),
                        };

                        let db_rw = match self.database_provider.provider_rw() {
                            Ok(db_rw) => db_rw,
                            Err(e) => {
                                // Panic bc this causes a db inconsistency:
                                // CometBFT has already committed the block so if
                                // the block can't be appended here, there will be an app hash
                                // mismatch. This requires a manual
                                // rollback to a healthy state.
                                panic!("Error getting database rw provider: {:?}", e);
                            }
                        };

                        db_rw.append_blocks_with_state(
                            vec![sealed_block_with_senders.clone()],
                            sealed_block_with_context.exec_outcome.clone(),
                            hashed_state.into_sorted(),
                            trie_updates,
                        )?;

                        db_rw.insert_staged_header(new_header.hash(), header_with_pegs)?;

                        db_rw.commit()?;

                        let new_chain = reth_chain_state::NewCanonicalChain::Commit {
                            new: vec![executed_block],
                        };
                        self.blockchain_provider
                            .canonical_in_memory_state()
                            .update_chain(new_chain);

                        self.blockchain_provider
                            .on_forkchoice_update_received(&ForkchoiceState::default());

                        self.blockchain_provider.set_canonical_head(new_header.clone());
                        self.blockchain_provider.set_safe(new_header.clone());
                        self.blockchain_provider.set_finalized(new_header.clone());

                        self.blockchain_provider
                            .canonical_in_memory_state()
                            .remove_persisted_blocks(block_height - 1);

                        debug!("eth block {block_height} committed to the state");

                        let chain = Chain::new(
                            vec![sealed_block_with_senders].into_iter(),
                            sealed_block_with_context.exec_outcome.clone(),
                            None,
                        );

                        self.blockchain_provider.canonical_in_memory_state().notify_canon_state(
                            CanonStateNotification::Commit {
                                new: Arc::new(chain),
                                pegins: Some(pegins),
                                pegouts: Some(pegouts),
                            },
                        );

                        if let Err(e) = commit_tx.send(()) {
                            error!("Failed to send await on channel for ABCIDriverMessage::CommitBlock message {e:?}");
                        }
                    }
                    ABCIDriverMessage::Exit => {
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{bitcoin_checkpoint::BitcoinCheckpoint, Storage};
    use bitcoin::{
        block::{BlockHash, Header, Version},
        hashes::Hash,
        CompactTarget, TxMerkleNode,
    };
    use botanix_btc_wallet::{
        bitcoind::{BitcoindConfig, BitcoindFactory},
        test_utils::MockBitcoindFactory,
    };
    use botanix_comet_bft_rpc::HttpCometBFTRpcClientFactory;
    use rand::thread_rng;
    use reth_chainspec::{BOTANIX_MAINNET, BOTANIX_TESTNET};
    use reth_cli_runner::tokio_runtime;
    use reth_db::{init_db, mdbx::DatabaseArguments};
    use reth_db_common::init::init_genesis;
    use reth_evm::test_utils::MockExecutorProvider;
    use reth_node_core::{args::TxPoolArgs, cli::config::RethTransactionPoolConfig};
    use reth_node_ethereum::EthEvmConfig;
    use reth_provider::providers::{ProviderFactory, StaticFileProvider};
    use reth_revm::primitives::EnvKzgSettings;
    use reth_tasks::TaskManager;
    use reth_transaction_pool::{
        blobstore::InMemoryBlobStore, test_utils::TransactionGenerator, EthPooledTransaction,
        EthTransactionValidator, Pool as RethPool, TransactionOrigin, TransactionPool,
        TransactionValidationTaskExecutor,
    };
    use std::path::Path;
    use tempfile::tempdir;
    use tendermint_abci::Application;
    use tendermint_proto::google::protobuf::Timestamp;

    type ABCIClientType = ABCIClient<
        MockExecutorProvider,
        MockBitcoindFactory,
        BlockchainProvider2<Arc<reth_db::DatabaseEnv>>,
        RethPool<
            TransactionValidationTaskExecutor<
                EthTransactionValidator<
                    BlockchainProvider2<Arc<reth_db::DatabaseEnv>>,
                    EthPooledTransaction,
                >,
            >,
            reth_transaction_pool::CoinbaseTipOrdering<EthPooledTransaction>,
            InMemoryBlobStore,
        >,
    >;

    /// Build the db and the ABCI client
    fn abci_client_builder() -> ABCIClientType {
        let secp = secp256k1::Secp256k1::new();
        let sk = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let pk = secp256k1::PublicKey::from_secret_key(&secp, &sk);

        // setup db client
        let temp_dir = tempdir().expect("to create temp dir");
        let db_path = temp_dir.path().join("db");
        let db_path = Path::new(&db_path);
        let db = Arc::new(init_db(db_path, DatabaseArguments::default()).expect("to init db"));
        let spec = Arc::new(BOTANIX_TESTNET.as_ref().to_owned());
        let factory = ProviderFactory::new(
            db.clone(),
            spec.clone(),
            StaticFileProvider::read_write(db_path.join("static_files"))
                .expect("static file providerto exist"),
        );
        let _ = init_genesis(factory.clone()).expect("to init genesis");
        let client =
            BlockchainProvider2::new(factory.clone()).expect("to create blockchain provider");

        let storage = Storage::new(
            Vec::new(),
            0,
            pk,
            bitcoin::Network::Regtest,
            Some(pk),
            Vec::new(),
            EthEvmConfig::default(),
            BOTANIX_TESTNET.clone(),
            MockBitcoindFactory::new(BitcoindConfig::default()),
            MockExecutorProvider::default(),
            client.clone(),
        );

        // setup validator with task executor
        let blob_store = InMemoryBlobStore::default();
        let tokio_runtime: tokio::runtime::Runtime = tokio_runtime().expect("to create runtime");
        let task_manager = TaskManager::new(tokio_runtime.handle().clone());
        let task_executor = task_manager.executor();
        let validator = TransactionValidationTaskExecutor::eth_builder(storage.chain_spec.clone())
            .with_head_timestamp(0)
            .kzg_settings(EnvKzgSettings::Default)
            .with_additional_tasks(1)
            .build_with_tasks(client.clone(), task_executor.clone(), blob_store.clone());

        let transaction_pool =
            RethPool::eth_pool(validator.clone(), blob_store, TxPoolArgs::default().pool_config());

        let bitcoin_checkpoints_chain =
            BitcoinCheckpointsChain::try_new(1, 0, 0).expect("create a valid chain");

        let bitcoin_header = Header {
            version: Version::default(),
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: TxMerkleNode::from_slice(&[0; 32])
                .expect("Failed to create merkle root from slice"),
            time: 0,
            bits: CompactTarget::from_consensus(0),
            nonce: 0,
        };

        let bitcoin_checkpoint = BitcoinCheckpoint::new(bitcoin_header, 0);
        bitcoin_checkpoints_chain.push(bitcoin_checkpoint).expect("push a checkpoint");

        let cometbft_rpc_factory = HttpCometBFTRpcClientFactory::default();

        let (driver_tx, _driver_rx) = tokio::sync::mpsc::channel(100);

        ABCIClient::new(
            storage,
            transaction_pool,
            Arc::new(bitcoin_checkpoints_chain),
            driver_tx,
            cometbft_rpc_factory,
            AuthorityConsensus::new(spec),
            false,
            Arc::new(AuthorityMetrics::default()),
            DataParser::default(),
            task_executor,
            factory,
            Arc::new(RwLock::new(SnapshotManagerStateLock::default())),
            Some(Arc::new(RwLock::new(SnapshotSyncStateLock::default()))),
            1,
            Some(Address::ZERO),
            client,
        )
    }

    fn non_deterministic_data_bytes(
        client: &ABCIClientType,
    ) -> Result<prost::bytes::Bytes, ConsensusError> {
        client
            .non_deterministic_data()
            .and_then(|ndd| client.serialize_non_deterministic_data_to_bytes(ndd))
    }

    #[test]
    #[should_panic(expected = "Chain ID mismatch")]
    fn test_init_chain_should_panic_if_chain_id_mismatch() {
        let abci_client = abci_client_builder();

        let request = RequestInitChain {
            chain_id: BOTANIX_MAINNET.chain.id().to_string(),
            ..Default::default()
        };
        let _ = abci_client.init_chain(request);
    }

    #[test]
    fn test_init_chain() {
        let abci_client = abci_client_builder();

        let request = RequestInitChain {
            chain_id: BOTANIX_TESTNET.chain.id().to_string(),
            ..Default::default()
        };
        let response = abci_client.init_chain(request);

        let expected_consensus_params = None;
        let expected_validators = vec![];

        assert_eq!(response.consensus_params, expected_consensus_params);
        assert_eq!(response.validators, expected_validators);
        let _response_app_hash_hex = hex::encode(response.app_hash.to_vec().as_slice());
        assert_eq!(
            response.app_hash.to_vec(),
            BOTANIX_TESTNET.genesis_hash.expect("Failed to unwrap genesis hash").0.to_vec()
        );
    }

    #[test]
    fn test_info() {
        let abci_client = abci_client_builder();

        let request = RequestInfo::default();
        let response = abci_client.info(request);

        assert_eq!(response.data, String::default());
        assert_eq!(response.version, VERSION.to_string());
        assert_eq!(response.app_version, 1);
        assert_eq!(response.last_block_height, 0);
        let _response_app_hash_hex = hex::encode(response.last_block_app_hash.to_vec().as_slice());
        assert_eq!(
            response.last_block_app_hash.to_vec(),
            BOTANIX_TESTNET.genesis_hash.expect("Failed to unwrap genesis hash").0.to_vec()
        );
    }

    #[test]
    fn test_prepare_proposal_empty_mempool() {
        let abci_client = abci_client_builder();

        let request = RequestPrepareProposal {
            max_tx_bytes: 100,
            time: Some(Default::default()),
            ..Default::default()
        };

        let response = abci_client.prepare_proposal(request);

        let expected_ndd = NonDeterministicData::new(
            abci_client.bitcoin_blockhash().expect("to have bitcoin blockhash"),
            abci_client.aggregate_public_key().expect("to have agg pk"),
            Address::ZERO,
        );
        let response_ndd_bytes = response.txs.first().expect("to have tx").clone();
        let reader_inner: Vec<u8> = vec![response_ndd_bytes].into_iter().flatten().collect();
        let reader = &mut io::Cursor::new(reader_inner);
        let response_ndd = NonDeterministicData::deserialize(reader).expect("to deserialize");

        assert_eq!(response.txs.len(), 1);
        assert_eq!(response_ndd, expected_ndd);
    }

    // TODO: fix error ValidationServiceUnreachable when adding tx to mempool
    #[test]
    #[ignore]
    fn test_prepare_proposal_tx_in_mempool() {
        let abci_client = abci_client_builder();

        let mut tx_generator = TransactionGenerator::new(thread_rng());
        let pooled_tx = tx_generator.gen_eip1559_pooled();

        let rt = tokio::runtime::Runtime::new().expect("to create runtime");

        rt.block_on(async {
            match abci_client.pool.add_transaction(TransactionOrigin::Local, pooled_tx).await {
                Ok(_) => {}
                Err(e) => {
                    panic!("Error adding tx to pool: {:?}", e);
                }
            }
        });

        let request = RequestPrepareProposal::default();
        let response = abci_client.prepare_proposal(request);

        let expected_ndd = NonDeterministicData::new(
            abci_client.bitcoin_blockhash().expect("to have agg bitcoin blockhash"),
            abci_client.aggregate_public_key().expect("to have agg pk"),
            Address::ZERO,
        );
        let response_ndd_bytes = response.txs.first().expect("to have tx").clone();
        let reader_inner: Vec<u8> = vec![response_ndd_bytes].into_iter().flatten().collect();
        let reader = &mut io::Cursor::new(reader_inner);
        let response_ndd = NonDeterministicData::deserialize(reader).expect("to deserialize");

        // todo: deserialize tx

        assert_eq!(response.txs.len(), 2);
        assert_eq!(response_ndd, expected_ndd);
    }

    #[test]
    fn test_process_proposal_with_ndd_tx_only() {
        let abci_client = abci_client_builder();

        let mut request = RequestProcessProposal::default();

        let ndd_bytes = non_deterministic_data_bytes(&abci_client).expect("to have ndd");

        request.txs = vec![ndd_bytes];

        let proposer_address = prost::bytes::Bytes::copy_from_slice(Address::ZERO.0.as_slice());
        request.proposer_address = proposer_address;

        request.time = Some(Timestamp::default());
        request.hash = prost::bytes::Bytes::copy_from_slice(FixedBytes::<32>::random().as_slice());

        let response = abci_client.process_proposal(request);

        assert_eq!(response.status, VERIFY_ACCEPTED);
    }

    #[test]
    fn test_process_proposal_with_signed_tx() {
        let abci_client = abci_client_builder();

        // first tx should be non-deterministic data
        let ndd_bytes = non_deterministic_data_bytes(&abci_client).expect("to have ndd");

        // second tx should be a signed transaction
        let mut tx_generator = TransactionGenerator::new(thread_rng());
        let signed_tx = tx_generator.transaction().into_legacy();
        let mut buf = Vec::new();
        signed_tx.encode_enveloped(&mut buf);
        let signed_tx_bytes = prost::bytes::Bytes::copy_from_slice(buf.as_slice());

        let request = RequestProcessProposal {
            txs: vec![ndd_bytes, signed_tx_bytes],
            proposer_address: prost::bytes::Bytes::copy_from_slice(Address::ZERO.0.as_slice()),
            time: Some(Timestamp::default()),
            hash: prost::bytes::Bytes::copy_from_slice(FixedBytes::<32>::random().as_slice()),
            ..Default::default()
        };

        let response = abci_client.process_proposal(request);

        // this fails bc prevrandao isn't being set in the evm env during tests
        // but all the custom code is executed successfully up to `build_and_execute`
        assert_eq!(response.status, VERIFY_REJECT);
    }

    #[test]
    fn test_finalize_block_with_ndd_tx_only() {
        let abci_client = abci_client_builder();

        let mut request = RequestFinalizeBlock::default();

        let ndd_bytes = non_deterministic_data_bytes(&abci_client).expect("to have ndd");

        request.txs = vec![ndd_bytes.clone()];

        let proposer_address = prost::bytes::Bytes::copy_from_slice(Address::ZERO.0.as_slice());
        request.proposer_address = proposer_address;

        request.time = Some(Timestamp::default());
        request.hash = prost::bytes::Bytes::copy_from_slice(FixedBytes::<32>::random().as_slice());

        let response = abci_client.finalize_block(request);

        // get newly made block from cache to recreate expected app hash
        let mut rw_lock = abci_client.block_cache.write().expect("should get lock");
        let sealed_block_with_context = rw_lock.cache.pop_newest().expect("to have block").1;
        let expected_app_hash = prost::bytes::Bytes::copy_from_slice(
            &sealed_block_with_context.sealed_block_with_peg.block().hash().0,
        );

        let expected_response = ResponseFinalizeBlock {
            events: vec![],
            tx_results: vec![ExecTxResult { code: SUCCESS, data: ndd_bytes, ..Default::default() }],
            validator_updates: vec![],
            consensus_param_updates: None,
            app_hash: expected_app_hash,
        };

        assert_eq!(response, expected_response);
    }

    // Test expected to fail bc the evm isn't fully setup in tests
    #[test]
    #[should_panic(expected = "Sender not found in state:")]
    fn test_finalize_block_with_signed_tx() {
        let abci_client = abci_client_builder();

        let mut request = RequestFinalizeBlock::default();

        // first tx should be non-deterministic data
        let ndd = abci_client.non_deterministic_data().expect("to have ndd");
        let ndd_bytes =
            abci_client.serialize_non_deterministic_data_to_bytes(ndd).expect("to serialize ndd");

        // second tx should be a signed transaction
        let mut tx_generator = TransactionGenerator::new(thread_rng());
        let signed_tx = tx_generator.transaction().into_legacy();
        let mut buf = Vec::new();
        signed_tx.encode_enveloped(&mut buf);
        let signed_tx_bytes = prost::bytes::Bytes::copy_from_slice(buf.as_slice());

        request.txs = vec![ndd_bytes.clone(), signed_tx_bytes];

        let proposer_address = prost::bytes::Bytes::copy_from_slice(Address::ZERO.0.as_slice());
        request.proposer_address = proposer_address;

        request.time = Some(Timestamp::default());
        request.hash = prost::bytes::Bytes::copy_from_slice(FixedBytes::<32>::random().as_slice());

        let response = abci_client.finalize_block(request);
        assert_eq!(response, ResponseFinalizeBlock::default());
    }

    #[test]
    fn test_snapshot_sync_state_equality() {
        let mut s1 = SnapshotSyncStateLock::default();
        s1.set_snapshot_height(100)
            .set_snapshot_chunks(30)
            .set_snapshot_format(1)
            .set_snapshot_hash(prost::bytes::Bytes::from("hash".as_bytes()));

        let mut s2 = SnapshotSyncStateLock::default();
        s2.set_snapshot_height(100)
            .set_snapshot_chunks(30)
            .set_snapshot_format(1)
            .set_snapshot_hash(prost::bytes::Bytes::from("hash2".as_bytes()));

        assert_ne!(s1, s2);

        s2.set_snapshot_hash(prost::bytes::Bytes::from("hash".as_bytes()));

        assert_eq!(s1, s2);
    }

    // TODO: add tests for commit + abci driver
    // https://github.com/botanix-labs/botanix/issues/907
}
