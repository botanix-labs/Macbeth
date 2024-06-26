use crate::{
    utils::retry_exec, AuthorityConsensus, Storage, StoragePBFT, BLOCK_TIME_DURATION_SECS,
};
use reth_consensus::Consensus;
use reth_consensus_common::utils::{is_inturn, unix_timestamp};
use reth_network::frost::manager::ToFrostManager;

use frost_secp256k1_tr as frost;

use reth_consensus_common::utils::current_inturn_index;
use reth_interfaces::{
    blockchain_tree::{BlockchainTreeEngine, BlockchainTreeViewer},
    executor::{BlockExecutionError, BlockValidationError},
    p2p::headers::client::HeadersClient,
};
use reth_network::frost::{
    manager::{peer_id_to_identifier, FrostCommand, FrostConfig},
    FrostPeerCommand, PbftEventResponseType, PbftResponse, PeerMessageResponse,
};
use reth_network_types::pk2id;
use reth_node_api::{error, ConfigureEvmEnv};
use reth_primitives::{
    botanix::BotanixConsensusPackage,
    extra_data_header::ExtraDataHeaderDeserializeError,
    header_ext::{BlockWitness, HeaderExt, RecoverAuthorityError, ValidateAuthoritySignatureError},
    BlockBody, BlockHash, BlockWithSenders, SealedBlock, TransactionSigned, U256,
};
use reth_provider::StateProvider;
use reth_provider::StateProviderFactory;
use reth_provider::{
    BlockExecutor, BlockReaderIdExt, ExecutorFactory, ProviderError, StateProviderBox,
};
use reth_revm::{database::StateProviderDatabase, processor::EVMProcessor, State};
use reth_rpc_types::PeerId;
use reth_tasks::TaskExecutor;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{
    mpsc::{error::SendError, UnboundedSender},
    RwLock,
};
use tracing::{debug, error, info, warn};

type SealedBlocksMap = Arc<RwLock<BTreeMap<BlockHash, SealedBlock>>>;
type PreCommitmentsMap = Arc<RwLock<BTreeMap<BlockHash, HashSet<PeerId>>>>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Failed to validate signatures on block: {0}")]
    InvalidSignature(#[from] ValidateAuthoritySignatureError),
    #[error("Failed to deserialize extra data header: {0}")]
    ExtraDataHeaderDeserializeError(#[from] ExtraDataHeaderDeserializeError),
    #[error("Failed to get connected peers handles")]
    FailedToGetConnectedPeersHandles,
    #[error("Missing signatures on block")]
    MissingSignatures,
    #[error("Missing in turn signature on block")]
    MissingInTurnSignature,
    #[error("Proposed block has too many signatures")]
    TooManySignaturesOnProposedBlock,
    #[error("Failed to recover signature: {0}")]
    RecoverSignatureError(#[from] secp256k1::Error),
    #[error("Failed to send peer command {0}")]
    Send(SendError<FrostPeerCommand>),
    #[error("Recieved block is not valid: {0}")]
    InvalidBlock(#[from] ValidateBlockError),
    #[error("Peer for time slot {0} already processed")]
    PeerAlreadyProcessedTimeSlot(u64),
    #[error("Recover authorities error {0}")]
    RecoverAuthoritiesError(#[from] RecoverAuthorityError),
    #[error("Block execution error: {0}")]
    BlockExecutionError(#[from] BlockExecutionError),
}

/// Error when validating a block as a block signer
#[derive(Debug, thiserror::Error)]
pub(crate) enum ValidateBlockError {
    #[error("Time check has been violated for blockhash: {0}")]
    TimecheckViolated(BlockHash),
    #[error("Could not find block in canonical chain: {0}")]
    ParentBlockNotFound(BlockHash),
    #[error("Fork is greater that 1 depth: {0}")]
    ForkDepthGreaterThanOne(BlockHash),
    #[error("Failed to deserialize extra data header: {0}")]
    ExtraDataHeaderDeserializeError(#[from] ExtraDataHeaderDeserializeError),
    #[error("Block is already in canon chain: {0}")]
    BlockAlreadyInCanonChain(BlockHash),
    #[error("Provider error: {0}")]
    ProviderError(#[from] reth_provider::ProviderError),
    #[error("Failed to find tip")]
    FailedToFindTip,
    #[error(
        "Parent hash known but is not canonnical tip, proposed block hash: {0}, parent hash: {1}"
    )]
    ParentHashNotCanonicalTip(BlockHash, BlockHash),
    #[error("Will not sign genesis block")]
    WillNotSignGenesisBlock,
    #[error("Block failed consensus check")]
    ConsensusCheckFailed,
}

impl PartialEq for ValidateBlockError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                ValidateBlockError::ParentHashNotCanonicalTip(a0, b0),
                ValidateBlockError::ParentHashNotCanonicalTip(a1, b1),
            ) => a0 == a1 && b0 == b1,
            (
                ValidateBlockError::ParentBlockNotFound(a),
                ValidateBlockError::ParentBlockNotFound(b),
            )
            | (
                ValidateBlockError::ForkDepthGreaterThanOne(a),
                ValidateBlockError::ForkDepthGreaterThanOne(b),
            )
            | (
                ValidateBlockError::BlockAlreadyInCanonChain(a),
                ValidateBlockError::BlockAlreadyInCanonChain(b),
            ) => a == b,
            (
                ValidateBlockError::WillNotSignGenesisBlock,
                ValidateBlockError::WillNotSignGenesisBlock,
            ) => true,
            _ => false,
        }
    }
}

/// Defines the states of the state machine
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PbftState {
    /// The initial dkg state
    Initial,
    /// Block proposed, now we are waiting for k pre-commitments from peers
    AwaitingPreCommitments,
    /// We have received k pre-commitments, now we are waiting for k commitments from peers
    AwaitingCommitments,
    /// finished state for either the block producer or the peer
    #[allow(dead_code)]
    /// TODO do we really need this?
    Finished,
}

impl PbftState {
    /// Returns true if the DKG state machine is in a running state
    pub(crate) fn is_running(&self) -> bool {
        match self {
            PbftState::Initial => false,
            _ => true,
        }
    }
    /// Returns true if we are waiting for a number of pre-commitments
    pub(crate) fn is_awaiting_precommitments(&self) -> bool {
        match self {
            PbftState::AwaitingPreCommitments => true,
            _ => false,
        }
    }
    /// Returns true if we are waiting for a number of pre-commitments
    pub(crate) fn is_awaiting_commitments(&self) -> bool {
        match self {
            PbftState::AwaitingCommitments => true,
            _ => false,
        }
    }
}

/// A state machine for transitioning between different DKG states
#[derive(Debug, Clone)]
pub(crate) struct PbftStateMachine<ToFrostMan: ToFrostManager, Client, NetworkClient, EF> {
    client: Client,
    storage: StoragePBFT,
    frost_handle: ToFrostMan,
    state: BTreeMap<BlockHash, PbftState>,
    /// our peer id
    peer_id: PeerId,
    config: FrostConfig,
    pre_commitments: Arc<RwLock<BTreeMap<BlockHash, HashSet<PeerId>>>>,
    sealed_blocks: Arc<RwLock<BTreeMap<BlockHash, SealedBlock>>>,
    secret_key: secp256k1::SecretKey,
    personal_frost_identifier: frost::Identifier,
    task_executor: Option<TaskExecutor>,
    network_client: NetworkClient,
    /// Store commitment to time slot
    time_slot_commitment: BTreeMap<u64, PeerId>,
    /// latest known bitcoin block header
    bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    consensus: AuthorityConsensus,
    /// executor factory
    executor_factory: EF,
}

impl<ToFrostMan: ToFrostManager, Client, NetworkClient, EF>
    PbftStateMachine<ToFrostMan, Client, NetworkClient, EF>
where
    EF: ExecutorFactory + Clone + 'static,
{
    /// Constructs a new state machine with the given params
    pub(crate) fn new(
        client: Client,
        storage: StoragePBFT,
        frost_handle: ToFrostMan,
        config: FrostConfig,
        peer_id: PeerId,
        secret_key: secp256k1::SecretKey,
        task_executor: Option<TaskExecutor>,
        network_client: NetworkClient,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        executor_factory: EF,
        consensus: AuthorityConsensus,
    ) -> Self {
        let personal_frost_identifier: frost::Identifier =
            peer_id_to_identifier(config.authority_index as u16);
        info!(
            target: "consensus::authority::pbft::new",
            "Frost identifier used: {:?} - {:?}",
            config.authority_index, personal_frost_identifier
        );

        Self {
            client,
            storage,
            personal_frost_identifier,
            frost_handle,
            state: BTreeMap::new(),
            config,
            peer_id,
            pre_commitments: Arc::new(RwLock::new(BTreeMap::new())),
            sealed_blocks: Arc::new(RwLock::new(BTreeMap::new())),
            time_slot_commitment: BTreeMap::new(),
            secret_key,
            task_executor: task_executor.clone(),
            network_client,
            executor_factory,
            bitcoin_block_header,
            consensus,
        }
    }

    /// Resets the state machine to its initial state
    pub(crate) fn reset(self) -> Self {
        Self {
            client: self.client,
            storage: self.storage,
            personal_frost_identifier: self.personal_frost_identifier,
            frost_handle: self.frost_handle,
            state: BTreeMap::new(),
            config: self.config,
            peer_id: self.peer_id,
            pre_commitments: Arc::new(RwLock::new(BTreeMap::new())),
            sealed_blocks: Arc::new(RwLock::new(BTreeMap::new())),
            secret_key: self.secret_key,
            task_executor: self.task_executor,
            network_client: self.network_client,
            time_slot_commitment: BTreeMap::new(),
            bitcoin_block_header: self.bitcoin_block_header,
            consensus: self.consensus,
            executor_factory: self.executor_factory,
        }
    }

    /// Returns the state machine state
    pub(crate) fn get_state(&self, block_hash: BlockHash) -> PbftState {
        self.state.get(&block_hash).unwrap_or(&PbftState::Initial).clone()
    }

    /// Sets state machine state
    pub(crate) fn set_state(&mut self, state: PbftState, block_hash: BlockHash) {
        // if the state doesnt exist for a block hash create it and set the state
        self.state.insert(block_hash, state);
    }
}

impl<ToFrostMan: ToFrostManager, Client, NetworkClient, EF>
    PbftStateMachine<ToFrostMan, Client, NetworkClient, EF>
where
    Client: StateProviderFactory + BlockReaderIdExt + BlockchainTreeViewer + Clone + 'static,
    ToFrostMan: ToFrostManager + Clone + 'static,
    NetworkClient: HeadersClient + Clone + 'static,
    EF: ExecutorFactory + Clone + 'static,
{
    pub(crate) async fn spawn_cleanup_task(&mut self) {
        let sleep_duration = Duration::from_secs(2 * BLOCK_TIME_DURATION_SECS);
        let client_clone = self.client.clone();
        let sealed_blocks: SealedBlocksMap = Arc::new(RwLock::new(BTreeMap::new()));
        let pre_commitments: PreCommitmentsMap = Arc::new(RwLock::new(BTreeMap::new()));
        let sealed_blocks_clone = Arc::clone(&sealed_blocks);
        let precommitments_clone: PreCommitmentsMap = Arc::clone(&pre_commitments);
        if let Some(task_exec) = self.task_executor.as_ref() {
            task_exec.spawn(async move {
                loop {
                    let tip = client_clone.canonical_tip();
                    let best_block_height = client_clone
                        .block_by_number(tip.number)
                        .ok()
                        .flatten()
                        .map(|b| b.header.number)
                        .unwrap_or_default();
                    let best_block_height = best_block_height.saturating_sub(2);

                    // find stale tx hashes
                    let guard = sealed_blocks_clone.read().await;
                    let stale_hashes = guard
                        .values()
                        .cloned()
                        .filter_map(|sealed_block| {
                            if sealed_block.header.number < best_block_height {
                                Some(sealed_block.hash())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>();
                    drop(guard);

                    // remove stale sealed blocks
                    let mut guard = sealed_blocks_clone.write().await;
                    guard.retain(|_, sealed_block| sealed_block.header.number >= best_block_height);
                    drop(guard);

                    // remove precommitments belonging to stale blocks
                    let mut guard = precommitments_clone.write().await;
                    guard.retain(|k, _| !stale_hashes.contains(k));
                    drop(guard);

                    // sleep until next cleanup round
                    tokio::time::sleep(sleep_duration).await;
                }
            });
        }
    }

    pub(crate) async fn get_all_peers_handle(
        &self,
    ) -> Result<HashMap<frost::Identifier, UnboundedSender<FrostPeerCommand>>, Error> {
        // get all frost peers connections
        let (peers_connections_sender, peers_connections_receiver) = tokio::sync::oneshot::channel::<
            HashMap<frost::Identifier, UnboundedSender<FrostPeerCommand>>,
        >();
        self.frost_handle
            .send_command(FrostCommand::GetAllConnectedFrostPeers(peers_connections_sender));
        match peers_connections_receiver.await {
            Ok(connected_peers) => Ok(connected_peers),
            Err(e) => {
                error!(target: "consensus::authority::pbft::get_all_peers_handle", "Failed to get frost peers connections {:?}", e);
                return Err(Error::FailedToGetConnectedPeersHandles);
            }
        }
    }

    pub(crate) fn is_coordinator(&self) -> bool {
        is_inturn(self.config.authorities.len() as u64, self.config.authority_index as u64)
    }

    pub(crate) async fn gossip_to_peers(
        &mut self,
        pbft_response: PbftResponse,
    ) -> Result<(), Error> {
        let fut = || async {
            // get all connected peers
            let connected_peers = self.get_all_peers_handle().await?;
            info!(target: "consensus::authority::pbft::gossip_to_peers","Broadcasting pbft response to all peers");
            info!(target: "consensus::authority::pbft::gossip_to_peers" ,"Connected peers: {:?}", connected_peers.keys().collect::<Vec<_>>() );

            // Broadcast dkg round 1 package to all peers (excluding ourselves)
            for (frost_id, sender) in connected_peers.iter() {
                if *frost_id != self.personal_frost_identifier {
                    sender
                        .send(FrostPeerCommand::PeerMessage(PeerMessageResponse::Pbft(
                            pbft_response.clone(),
                        )))
                        .map_err(Error::Send)?;
                }
            }
            Ok(())
        };

        retry_exec(fut, 3, Duration::from_secs(1)).await
    }

    async fn validate_block(&self, block_to_sign: &SealedBlock) -> Result<(), ValidateBlockError> {
        // Should never sign genesis block
        if block_to_sign.header.number == 0 {
            return Err(ValidateBlockError::WillNotSignGenesisBlock);
        }

        let block_hash = block_to_sign.header.segregated_signature_block_hash()?;
        block_to_sign
            .header
            .validate_inturn(&self.config.authorities)
            .map_err(|_| ValidateBlockError::TimecheckViolated(block_hash))?;

        // Blocks should only be signed if they are building on the best block
        // Or building on one of the 1 block deep forks
        // But never on a block that is not in the canonical chain
        // Or a block building on a fork that is deeper than 1 block deep
        let tip = self.client.canonical_tip();
        let best_block =
            self.client.block_by_number(tip.number)?.ok_or(ValidateBlockError::FailedToFindTip)?;
        let best_block_hash = tip.hash;

        // if the suggested block is the canon tip there is no point to signing it again
        match self.client.is_canonical(block_hash) {
            Ok(false) => (),                                // continue
            Err(ProviderError::BlockHashNotFound(_)) => (), /* great block being proposed is not */
            // canon
            _ => return Err(ValidateBlockError::BlockAlreadyInCanonChain(block_hash)),
        }

        // Check if we are building on a block that is in the canonical chain
        // or a fork
        if block_to_sign.parent_hash == best_block_hash {
            return Ok(());
        }
        // TODO re-consider if this is possible
        else if best_block.header.number == 0 {
            // Case where the best block is the genesis block
            // This should never happen
            return Ok(());
        } else {
            // Somehow we have the parent block but its not the current canon chain?
            // This should not happen
            if self.client.contains(block_to_sign.parent_hash) {
                return Err(ValidateBlockError::ParentHashNotCanonicalTip(
                    block_to_sign.hash_slow(),
                    block_to_sign.parent_hash,
                ));
            }

            // we could be missing the parent block that is being suggested indicating that there is
            // a fork retrieve the missing block via the network client.
            // if that retrieved block's parent is not the best block's parent hash then the fork is
            // deeper than 1 block and we do not sign
            // TODO does the peer that we are getting this block from matter?
            match self
                .network_client
                .get_header(reth_rpc_types::BlockHashOrNumber::Hash(block_to_sign.parent_hash))
                .await
            {
                Ok(header_with_peer_id) => {
                    if let Some(header) = header_with_peer_id.1 {
                        if header.parent_hash != best_block.parent_hash {
                            return Err(ValidateBlockError::ForkDepthGreaterThanOne(block_hash));
                        }
                    } else {
                        return Err(ValidateBlockError::ParentBlockNotFound(
                            block_to_sign.parent_hash,
                        ));
                    }
                }
                Err(e) => {
                    error!(target: "consensus::authority::pbft::validate_block", "Failed to get header for block: {:?}", e);
                    return Err(ValidateBlockError::ParentBlockNotFound(block_to_sign.parent_hash));
                }
            }
            return Ok(());
        }
    }

    /// Execute and run poa validation on the block without updating state or inserting it into the storage
    pub(crate) async fn execute_and_validate_poa_consensus(
        &mut self,
        block: SealedBlock,
    ) -> Result<(), BlockExecutionError> {
        let recent_bitcoin_block_header = *self.bitcoin_block_header.read().await;
        let botanix_consensus_pkg = Some(BotanixConsensusPackage {
            recent_header: recent_bitcoin_block_header.expect("recent header to exist"),
            aggregate_public_key: self
                .storage
                .aggregate_public_key
                .clone()
                .expect("aggregate pk is some"),
            btc_network: self.storage.btc_network,
        });

        let senders = TransactionSigned::recover_signers(&block.body, block.body.len())
            .ok_or(BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError))?;

        let block_with_senders = BlockWithSenders::new(block.clone().unseal(), senders.clone())
            .expect("senders are valid");

        // validate before executing block
        let authority_signers = self.storage.authorities.clone();
        let genesis_authorities = self.storage.genesis_authorities.clone();
        self.consensus
            .validate_header_standalone(
                &block.header.clone(),
                &authority_signers,
                &genesis_authorities,
                true,
            )
            .map_err(|e| {
                warn!(target: "consensus::authority", "failed to validate POA header during PBFT: {:?}", e);
                e
            })?;
        let db = self.client.latest().expect("get latest");
        let mut executor = self.executor_factory.with_state(&db);

        executor.execute_transactions(&block_with_senders, U256::ZERO, botanix_consensus_pkg)?;

        Ok(())
    }

    /// Intended to be called by the in turn block producer when a block is ready to be
    /// proposed to the network
    /// Note: there should already be a signature on the block at this point
    pub(crate) async fn init_block_proposal(&mut self, block: SealedBlock) -> Result<(), Error> {
        // Check if there is already a running state machine for this block
        let block_hash = block.header.segregated_signature_block_hash()?;
        let current_state = self.get_state(block_hash);

        if current_state.is_running() {
            warn!(target: "consensus::authority::pbft::init_block_proposal" ,"State machine is already running for block {:?}", block_hash);
            return Ok(());
        }

        if !self.is_coordinator() {
            warn!(target: "consensus::authority::pbft::init_block_proposal" ,"Not the coordinator -- ignoring init block proposal request");
            return Ok(());
        }

        if block.header.deserialize_extra_data_header()?.authority_signatures.is_none() {
            warn!(target: "consensus::authority::pbft::init_block_proposal" ,"Block proposal does not contain any signatures");
            return Err(Error::MissingSignatures);
        }

        // Save block locally
        self.sealed_blocks.write().await.insert(block_hash, block.clone());

        // Set the state to awaiting pre-commitments
        self.set_state(PbftState::AwaitingPreCommitments, block_hash);
        self.gossip_to_peers(PbftResponse {
            response_type: PbftEventResponseType::CoordinatorBlockProposal,
            data: block.clone(),
        })
        .await?;

        // As the coordinator we can add our own pre-commitment
        self.pre_commitments
            .write()
            .await
            .entry(block_hash)
            .or_insert_with(HashSet::new)
            .insert(self.peer_id);

        Ok(())
    }

    /// Block proposal from the in turn block producer
    /// We should only be getting this request from the in turn block producer
    pub(crate) async fn process_block_proposal(
        &mut self,
        block: SealedBlock,
        peer_id: PeerId,
    ) -> Result<(), Error> {
        info!(target: "consensus::authority::pbft::process_block_proposal" ,"Processing block proposal from peer {:?}", peer_id);
        let block_hash = block.header.segregated_signature_block_hash()?;
        let current_state = self.get_state(block_hash);
        if current_state.is_running() {
            warn!(target: "consensus::authority::pbft::process_block_proposal" ,"State machine is already running for block {:?}", block_hash);
            return Ok(());
        }

        if peer_id == self.peer_id {
            return Ok(());
        }

        // validate block
        self.validate_block(&block).await?;

        let coordinator = self
            .config
            .authorities
            .get(current_inturn_index(self.config.authorities.len() as u64, unix_timestamp())
                as usize)
            .expect("should be valid index");

        // Check if the inturn block producer has the first signature on the block
        // this serves as authentication that the block was produced by the in turn block producer
        match block.header.deserialize_extra_data_header()?.authority_signatures {
            Some(sigs) => {
                if sigs.len() > 1 {
                    return Err(Error::TooManySignaturesOnProposedBlock);
                }
                let msg = secp256k1::Message::from_digest_slice(
                    &block.header.create_sighash()?.0.as_slice(),
                )?;
                let recovered_pk = sigs[0].recover(&msg)?;
                if recovered_pk != *coordinator {
                    warn!(target: "consensus::authority::pbft::process_block_proposal" ,"In turn block producer does not have the first signature on the block");
                    return Err(Error::MissingInTurnSignature);
                }
            }
            None => {
                warn!(target: "consensus::authority::pbft::process_block_proposal" ,"Block proposal does not contain any signatures");
                return Err(Error::MissingSignatures);
            }
        }

        // execute block and run poa consensus
        self.execute_and_validate_poa_consensus(block.clone()).await?;

        // Add our own pre-commitment
        let mut pre_commits = HashSet::new();
        pre_commits.insert(self.peer_id);
        // And implicitly add the coordinator's pre-commitment
        pre_commits.insert(peer_id);
        self.pre_commitments.write().await.insert(block_hash, pre_commits);
        self.set_state(PbftState::AwaitingPreCommitments, block_hash);

        // Broadcast our pre-commitment
        self.gossip_to_peers(PbftResponse {
            response_type: PbftEventResponseType::PeerPreCommitment,
            data: block.clone(),
        })
        .await?;

        // Edge case: In a two person federation we can move to the next state
        self.check_and_send_commitment(&block, &peer_id).await?;

        Ok(())
    }

    /// Check if we have enough pre-commits to move onto the next state
    /// If we do, we can send our commitment
    pub(crate) async fn check_and_send_commitment(
        &mut self,
        block: &SealedBlock,
        _peer_id: &PeerId,
    ) -> Result<(), Error> {
        let block_hash = block.header.segregated_signature_block_hash()?;
        let signed_authorities = block.header.recovered_signed_authorities()?;
        // Check if we have already signed for this time slot
        let time_slot = block.header.timestamp / 60;
        let coord_pk = signed_authorities.get(0).unwrap();
        let coord_peer_id = pk2id(&coord_pk);

        if let Some(peer) = self.time_slot_commitment.get(&time_slot) {
            if *peer == coord_peer_id {
                warn!(target: "consensus::authority::pbft::check_and_send_commitment" ,"Peer has already processed this time slot");
                return Err(Error::PeerAlreadyProcessedTimeSlot(time_slot));
            }
        }

        let pre_commits = self
            .pre_commitments
            .read()
            .await
            .get(&block_hash)
            .cloned()
            .unwrap_or_else(HashSet::new);
        // if we have enough precommitments, we can move to the next state
        if pre_commits.len() >= self.config.max_signers as usize {
            // Save that we processed this time slot from this peer
            let time_slot = block.header.timestamp / 60;
            info!(target: "consensus::authority::pbft::check_and_send_commitment" ,"We have enough pre-commitments moving to next state");
            let mut mutable_header = block.header().clone();
            mutable_header.sign_block(&self.secret_key)?;
            let signed_block = SealedBlock::new(
                mutable_header.seal_slow(),
                BlockBody { transactions: block.body.clone(), ommers: vec![], withdrawals: None },
            );
            self.time_slot_commitment.insert(time_slot, coord_peer_id);
            // Update state
            self.set_state(PbftState::AwaitingCommitments, block_hash);
            // Gossip our commitment
            self.gossip_to_peers(PbftResponse {
                response_type: PbftEventResponseType::PeerCommitment,
                data: signed_block.clone(),
            })
            .await?;
        }

        Ok(())
    }

    pub(crate) async fn process_precommitment(
        &mut self,
        block: SealedBlock,
        peer_id: PeerId,
    ) -> Result<(), Error> {
        info!(target: "consensus::authority::pbft::process_precommitment", "Processing pre-commitment from peer {:?}", peer_id);
        self.validate_block(&block).await?;

        let block_hash = block.header.segregated_signature_block_hash()?;
        let current_state = self.get_state(block_hash);
        if !current_state.is_awaiting_precommitments() {
            warn!(target: "consensus::authority::pbft::process_precommitment", "State machine is not awaiting pre-commitments for block {:?}", block_hash);
            return Ok(());
        }

        // Do not process our own response
        if peer_id == self.peer_id {
            return Ok(());
        }

        // execute block and run poa consensus
        self.execute_and_validate_poa_consensus(block.clone()).await?;

        // Add the peer's precommitment
        let mut write_handle = self.pre_commitments.write().await;
        let pre_commits = write_handle.entry(block_hash).or_insert_with(HashSet::new);
        pre_commits.insert(peer_id);
        info!(target: "consensus::authority::pbft::process_precommitment" ,"pre-commitments: {:?}", pre_commits.len());
        drop(write_handle);

        self.check_and_send_commitment(&block, &peer_id).await?;

        Ok(())
    }

    /// Process a commitment from a peer
    /// If we have enough commitments, returns true
    /// Otherwise returns false
    pub(crate) async fn process_commitment(
        &mut self,
        block: SealedBlock,
        peer_id: PeerId,
    ) -> Result<Option<BlockWitness>, Error> {
        self.validate_block(&block).await?;
        // Only the in turn coordinator should be processing commitments
        if !self.is_coordinator() {
            warn!(target: "consensus::authority::pbft::process_commitment" ,"Not the coordinator -- ignoring commitment from peer {:?}", peer_id);
            return Ok(None);
        }
        if peer_id == self.peer_id {
            return Ok(None);
        }

        // execute block and run poa consensus
        self.execute_and_validate_poa_consensus(block.clone()).await?;

        let block_hash = block.header.segregated_signature_block_hash()?;
        // Check that this peer specifically provided a signature
        let current_state = self.get_state(block_hash);
        if !current_state.is_awaiting_commitments() {
            warn!(target: "consensus::authority::pbft::process_commitment" ,"State machine is not awaiting commitments for block {:?}", block_hash);
            return Ok(None);
        }

        let lock = self.sealed_blocks.read().await;
        // This block is originally added during init block proposal
        let current_block = lock
            .get(&block_hash)
            // TODO should we be error'ing here instead
            .expect("block should exist")
            .clone();
        drop(lock);
        let mut current_header = current_block.header().clone();
        let mut edh = current_header.deserialize_extra_data_header()?;
        let peer_edh = block.header().deserialize_extra_data_header()?;

        if peer_edh.authority_signatures.is_none() {
            debug!(target: "consensus::authority::pbft::process_commitment" ,"Peer did not provide a signature");
            return Ok(None);
        }

        // Check that the commited block is the same as the block we are tracking
        if current_header.segregated_signature_block_hash()?
            != block.header.segregated_signature_block_hash()?
        {
            warn!(target: "consensus::authority::pbft::process_commitment" ,"Block hash recieved from peer does not match the block we are tracking");
            return Ok(None);
        }
        // Check all the signatures on the commited block from the peer
        block.header().check_authority_sig_add(&self.config.authorities)?;

        // Should merge this peers siganture into the main block where we are tracking all
        // signatures If that signature provided is not valid fail
        // If they did not provide a sig fail
        // merge signature from peer
        edh.merge_signature(&peer_edh);
        // update header

        current_header.add_extra_data_header(&edh);
        // TODO do we need to clone here
        let mut new_block = current_block.clone();
        new_block.header = current_header.seal(block_hash);
        // Update local state
        self.sealed_blocks.write().await.insert(block_hash, new_block.clone());
        let number_of_valid_sigs =
            new_block.header().check_authority_sig_add(&self.config.authorities)?;
        info!("number of valid sigs: {}", number_of_valid_sigs);
        info!("max signers: {}", self.config.max_signers);
        // if we have enough commitments, we can move to the next state
        if number_of_valid_sigs >= self.config.max_signers {
            info!(target: "consensus::authority::pbft::process_commitment" ,"We have enough commitments, time to produce a block");
            let block_witness =
                new_block.header().get_block_witness()?.expect("set the witness above");
            info!(target: "consensus::authority::pbft::process_commitment" ,"Block witness: {:?}", block_witness);
            return Ok(Some(block_witness));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    #![allow(unused_mut)]
    use super::*;
    use bitcoin::{
        block::{Header as BitcoinHeader, Version},
        hash_types::TxMerkleNode,
        hashes::Hash,
        BlockHash, CompactTarget,
    };
    use rand;
    use reth_consensus_common::utils::unix_timestamp;
    use reth_interfaces::p2p::{
        download::DownloadClient,
        error::{PeerRequestResult, RequestError},
        headers::client::HeadersRequest,
        priority::Priority,
    };
    use reth_network::frost::manager::ToFrostManager;
    use reth_network_types::{pk2id, WithPeerId};
    use reth_primitives::{extra_data_header::ExtraDataHeader, Header, B256, BOTANIX_TESTNET};
    use reth_provider::{
        test_utils::{MockEthProvider, TestExecutorFactory},
        HeaderProvider,
    };
    use secp256k1::SECP256K1;

    #[derive(Clone, Debug)]
    pub(crate) struct MockNetworkClient {
        pub(crate) client: MockEthProvider,
    }

    impl MockNetworkClient {
        pub(crate) fn new(provider: MockEthProvider) -> Self {
            Self { client: provider }
        }
    }

    impl DownloadClient for MockNetworkClient {
        fn report_bad_message(&self, _peer_id: PeerId) {
            unimplemented!()
        }

        fn num_connected_peers(&self) -> usize {
            unimplemented!()
        }
    }

    impl HeadersClient for MockNetworkClient {
        type Output = futures_util::future::Ready<PeerRequestResult<Vec<Header>>>;

        fn get_headers_with_priority(
            &self,
            request: HeadersRequest,
            _priority: Priority,
        ) -> Self::Output {
            // let headers = self.client.headers.lock();
            match self.client.header_by_hash_or_number(request.start) {
                Ok(header_res) => {
                    if let Some(header) = header_res {
                        return futures_util::future::ready(PeerRequestResult::Ok(
                            WithPeerId::new(PeerId::random(), vec![header]),
                        ));
                    }
                }
                // Error is caught below
                Err(_) => (),
            }

            futures_util::future::ready(PeerRequestResult::Err(RequestError::BadResponse))
        }
    }

    macro_rules! setup_multi_party_test {
        ($n:expr, $sks:ident, $frost_handle_mock:ident, $configs:ident, $peer_ids:ident, $signed_blocks:ident, $non_coords:ident, $coord:ident, $block_to_propose:ident, $mock_eth_provider:ident, $mock_network_client:ident) => {
            let secp = secp256k1::Secp256k1::new();
            let mut $mock_eth_provider = MockEthProvider::default();
            let mut $mock_network_client = MockNetworkClient::new($mock_eth_provider.clone());

            let mut $sks = vec![];
            let mut $configs = vec![];
            let mut $peer_ids = vec![];
            // redundant to define this again ends up being neater
            let mut pks = vec![];
            let mut $signed_blocks = vec![];

            let $frost_handle_mock = FrostHandleMock {};
            for _ in 0..$n {
                let sk = secp256k1::SecretKey::new(&mut rand::thread_rng());
                let pk = secp256k1::PublicKey::from_secret_key(&secp, &sk);
                $sks.push(sk);
                let peer_id = pk2id(&pk);
                $peer_ids.push(peer_id);

                pks.push(pk);
            }

            for i in 0..$n {
                let pk = pks[i];
                let config = FrostConfig {
                    authorities: pks.clone(),
                    authority_index: i,
                    max_signers: $n,
                    min_signers: $n,
                    authority_pk: pk,
                };
                $configs.push(config);
            }

            // set up parent block
            let mut parent_header = Header::default();
            // Set the nonce to 1 so the block hash is not default block hash
            parent_header.nonce = 1u64;
            let parent_block = SealedBlock::new(parent_header.seal_slow(), BlockBody::default());
            $mock_eth_provider.add_block(parent_block.hash_slow(), parent_block.clone().into());

            let ts = unix_timestamp();
            let authorities = pks.clone();
            for i in 0..$n {
                let mut edh = ExtraDataHeader::default();
                edh.authority_signers = Some(authorities.clone());
                let mut header = Header::default();
                header.number = 1;
                header.parent_hash = parent_block.hash_slow();
                header.timestamp = ts;
                header.base_fee_per_gas = Some(1);
                header.add_extra_data_header(&edh);
                header.sign_block(&$sks[i]).unwrap();
                let block_body = BlockBody::default();
                $signed_blocks.push(SealedBlock::new(header.seal_slow(), block_body));
            }

            let mut $non_coords = vec![];
            let mut $block_to_propose = None;
            let mut $coord = None;

            let header = BitcoinHeader {
                version: Version::default(),
                prev_blockhash: BlockHash::all_zeros(),
                merkle_root: TxMerkleNode::all_zeros(),
                time: 0,
                bits: CompactTarget::default(),
                nonce: 0,
            };
            let bitcoin_block_header = Arc::new(RwLock::new(Some((header, 0))));
            let storage = StoragePBFT::new(
                authorities.clone(), // genesis authorities
                authorities.clone(), // authorities
                Some(pks[0]),
                bitcoin::Network::from_core_arg("regtest").expect("regtest exists"),
            );

            for i in 0..$n {
                let pbft_state_machine = PbftStateMachine::new(
                    $mock_eth_provider.clone(),
                    storage.clone(),
                    $frost_handle_mock.clone(),
                    $configs[i].clone(),
                    $peer_ids[i],
                    $sks[i],
                    None,
                    $mock_network_client.clone(),
                    bitcoin_block_header.clone(),
                    TestExecutorFactory::default(),
                    AuthorityConsensus::new(Arc::clone(&BOTANIX_TESTNET.clone())),
                );
                if !pbft_state_machine.is_coordinator() {
                    $non_coords.push(pbft_state_machine.clone());
                } else {
                    $coord = Some(pbft_state_machine.clone());
                    $block_to_propose = Some($signed_blocks[i].clone());
                }
            }
            let mut $block_to_propose = $block_to_propose.expect("should have a block to propose");
            let mut $coord = $coord.expect("should have a coordinator");
        };
    }
    /* Tests for PbftStateMachine */
    // mock frost handle
    #[derive(Clone)]
    struct FrostHandleMock;
    impl ToFrostManager for FrostHandleMock {
        fn send_command(&self, command: FrostCommand) {
            match command {
                FrostCommand::CheckConnectedToAll(sender) => sender.send(true).unwrap(),
                FrostCommand::GetAllConnectedFrostPeers(sender) => {
                    let peers = HashMap::new();
                    sender.send(peers).unwrap();
                }
                FrostCommand::GetPeerMessagesStream(_sender) => {
                    // let (tx, _) = tokio::sync::mpsc::unbounded_channel();
                    // sender.send(tx).unwrap();
                }
            }
        }
    }

    #[test]
    fn test_pbft_state_machine_new() {
        setup_multi_party_test!(
            1,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            _block_to_propose,
            mock_eth_provider,
            mock_network_client
        );

        let pbft_state_machine = coord;
        // Check that the initial state is empty
        assert!(pbft_state_machine.state.is_empty());
    }

    #[tokio::test]
    async fn init_block_proposal() {
        setup_multi_party_test!(
            1,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            block_to_propose,
            mock_eth_provider,
            mock_network_client
        );
        let pbft_state_machine = coord;
        let block_hash = block_to_propose
            .header()
            .segregated_signature_block_hash()
            .expect("to get the block hash");
        // if the state is not init for this block hash it should fail
        // pbft_state_machine.set_state(PbftState::AwaitingCommitments, block_hash);
        // pbft_state_machine.init_block_proposal(block_to_propose.clone()).await.expect("valid
        // block proposal");

        // reset and this time the state should be waiting for pre-commitments
        let mut pbft_state_machine = pbft_state_machine.reset();
        pbft_state_machine
            .init_block_proposal(block_to_propose.clone())
            .await
            .expect("valid block proposal");
        assert_eq!(
            pbft_state_machine.sealed_blocks.read().await.get(&block_hash).unwrap(),
            &block_to_propose
        );
        // there should only be the one pre-commitment
        assert_eq!(
            pbft_state_machine.pre_commitments.read().await.get(&block_hash).unwrap().len(),
            1
        );
        assert_eq!(pbft_state_machine.get_state(block_hash), PbftState::AwaitingPreCommitments);

        // Since we are now waiting for pre commitments it should not change the state
        pbft_state_machine
            .init_block_proposal(block_to_propose.clone())
            .await
            .expect("valid block proposal");
        assert_eq!(
            pbft_state_machine.sealed_blocks.read().await.get(&block_hash).unwrap(),
            &block_to_propose
        );
        // there should only be the one pre-commitment
        assert_eq!(
            pbft_state_machine.pre_commitments.read().await.get(&block_hash).unwrap().len(),
            1
        );
        assert_eq!(pbft_state_machine.get_state(block_hash), PbftState::AwaitingPreCommitments);

        // Re initialing with the same block proposal should not change the state
        pbft_state_machine.set_state(PbftState::Initial, block_hash);
        pbft_state_machine
            .init_block_proposal(block_to_propose.clone())
            .await
            .expect("valid block proposal");
        assert_eq!(
            pbft_state_machine.sealed_blocks.read().await.get(&block_hash).unwrap(),
            &block_to_propose
        );
        // there should only be the one pre-commitment
        assert_eq!(
            pbft_state_machine.pre_commitments.read().await.get(&block_hash).unwrap().len(),
            1
        );
        assert_eq!(pbft_state_machine.get_state(block_hash), PbftState::AwaitingPreCommitments);
    }

    #[ignore]
    #[tokio::test]
    async fn will_reprocess_proposal_for_timeslot() {
        setup_multi_party_test!(
            2,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            block_to_propose,
            mock_eth_provider,
            mock_network_client
        );

        let block_hash = block_to_propose
            .header()
            .segregated_signature_block_hash()
            .expect("to get the block hash");

        non_coords[0]
            .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
            .await
            .expect("valid block proposal");
        let other_peer = non_coords.get_mut(0).unwrap();

        let lock = other_peer.pre_commitments.read().await;
        let pre_commits = lock.get(&block_hash).expect("to get pre-commits");
        assert_eq!(pre_commits.len(), 2);
        drop(lock);

        let time_slots = &other_peer.clone().time_slot_commitment;
        assert_eq!(time_slots.len(), 1);
        // only timeslot should be coord peerid
        assert_eq!(time_slots.iter().next().unwrap().1, &coord.peer_id);

        let res =
            other_peer.clone().check_and_send_commitment(&block_to_propose, &coord.peer_id).await;
        // TODO should be checking an error variant
        assert!(res.err().unwrap().to_string().contains("Peer for time slot"));

        // Re-procesing with different time stamp should be fine
        let edh = ExtraDataHeader::default();
        let mut header_to_sign = Header::default();
        header_to_sign.number = 1;
        // Still "in turn" but different time slot
        header_to_sign.timestamp = unix_timestamp() * 2;
        header_to_sign.add_extra_data_header(&edh);
        header_to_sign.sign_block(&coord.secret_key).expect("to sign block");
        let block = SealedBlock::new(header_to_sign.seal_slow(), BlockBody::default());
        other_peer
            .clone()
            .process_block_proposal(block.clone(), coord.peer_id.clone())
            .await
            .expect("valid block proposal");

        assert_eq!(time_slots.len(), 1);
        // only timeslot should be coord peerid
        assert_eq!(time_slots.iter().next().unwrap().1, &coord.peer_id);
    }

    #[tokio::test]
    async fn test_block_proposal_cannot_add_self() {
        setup_multi_party_test!(
            1,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            block_to_propose,
            mock_eth_provider,
            mock_network_client
        );
        let mut pbft_state_machine = coord;
        let block_hash = block_to_propose
            .header()
            .segregated_signature_block_hash()
            .expect("to get the block hash");
        // Should not add a block from ourselves
        pbft_state_machine
            .process_block_proposal(block_to_propose.clone(), pbft_state_machine.peer_id.clone())
            .await
            .expect("valid block proposal");
        assert_eq!(pbft_state_machine.sealed_blocks.read().await.get(&block_hash), None);
        assert_eq!(pbft_state_machine.pre_commitments.read().await.get(&block_hash), None);
        assert_eq!(pbft_state_machine.get_state(block_hash), PbftState::Initial);
    }

    #[tokio::test]
    async fn test_cannot_propose_non_coord_block() {
        // Note: set up test signs with the first authorities key
        setup_multi_party_test!(
            2,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            _block_to_propose,
            mock_eth_provider,
            mock_network_client
        );

        let tip = mock_eth_provider.canonical_tip();
        // sign the block as the non-coordinator
        let non_coord_sk = non_coords[0].secret_key.clone();
        let edh = ExtraDataHeader::default();
        let mut invalid_block_header = Header::default();
        invalid_block_header.parent_hash = tip.hash;
        invalid_block_header.number = 1;
        invalid_block_header.timestamp = unix_timestamp();
        invalid_block_header.add_extra_data_header(&edh);
        invalid_block_header.sign_block(&non_coord_sk).expect("to sign block");
        let invalid_block =
            SealedBlock::new(invalid_block_header.seal_slow(), BlockBody::default());
        // try to propose an a block singed by a non coord
        let res = non_coords[0]
            .process_block_proposal(invalid_block.clone(), coord.peer_id.clone())
            .await;
        assert!(res
            .err()
            .unwrap()
            .to_string()
            .contains("Time check has been violated for blockhash"));
    }

    #[tokio::test]
    async fn test_cannot_propose_block_with_two_signatures() {
        // Note: set up test signs with the first authorities key
        setup_multi_party_test!(
            2,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            block_to_propose,
            mock_eth_provider,
            mock_network_client
        );

        // sign the block as the non-coordinator
        let non_coord_sk = non_coords[0].secret_key.clone();
        let mut invalid_block_header = block_to_propose.header().clone();
        invalid_block_header.sign_block(&non_coord_sk).expect("to sign block");

        let invalid_block =
            SealedBlock::new(invalid_block_header.seal_slow(), BlockBody::default());
        // try to propose an a block singed by a non coord
        let res = non_coords[0]
            .process_block_proposal(invalid_block.clone(), coord.peer_id.clone())
            .await;
        assert!(res.is_err());
        assert_eq!(res.err().unwrap().to_string(), "Proposed block has too many signatures");
    }

    #[ignore]
    #[tokio::test]
    async fn test_two_party_block_propose_flow() {
        // Note: set up test signs with the first authorities key
        setup_multi_party_test!(
            2,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            block_to_propose,
            mock_eth_provider,
            mock_network_client
        );

        // Propose valid block and assert correct state transitions
        let block_hash = block_to_propose
            .header()
            .segregated_signature_block_hash()
            .expect("to get the block hash");

        non_coords[0]
            .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
            .await
            .expect("valid block proposal");
        // There should be two commitments
        assert_eq!(non_coords[0].pre_commitments.read().await.get(&block_hash).unwrap().len(), 2);
        // One for the peer that sent their pre commitment
        assert!(non_coords[0]
            .pre_commitments
            .read()
            .await
            .get(&block_hash)
            .unwrap()
            .contains(&peer_ids[1]));
        // Another implicitly added for the coord that proposed the block
        assert!(non_coords[0]
            .pre_commitments
            .read()
            .await
            .get(&block_hash)
            .unwrap()
            .contains(&peer_ids[0]));
        // at this point we have two commitments from all peers we should be awaiting commitments
        assert!(non_coords[0].get_state(block_hash).is_awaiting_commitments());

        // Getting anther block proposal from the same peer should not change the state
        non_coords[0]
            .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
            .await
            .expect("valid block proposal");
        assert_eq!(non_coords[0].pre_commitments.read().await.get(&block_hash).unwrap().len(), 2);
        assert!(non_coords[0]
            .pre_commitments
            .read()
            .await
            .get(&block_hash)
            .unwrap()
            .contains(&peer_ids[1]));
        assert!(non_coords[0]
            .pre_commitments
            .read()
            .await
            .get(&block_hash)
            .unwrap()
            .contains(&peer_ids[0]));
        assert!(non_coords[0].get_state(block_hash).is_awaiting_commitments());
    }

    #[ignore]
    #[tokio::test]
    async fn test_three_party_block_propose_flow() {
        // Note: set up test signs with the first authorities key
        setup_multi_party_test!(
            3,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            block_to_propose,
            mock_eth_provider,
            mock_network_client
        );

        // Propose valid block and assert correct state transitions
        let block_hash = block_to_propose
            .header()
            .segregated_signature_block_hash()
            .expect("to get the block hash");

        non_coords[0]
            .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
            .await
            .expect("valid block proposal");
        // There should be two commitments
        assert_eq!(non_coords[0].pre_commitments.read().await.get(&block_hash).unwrap().len(), 2);
        assert!(non_coords[0].get_state(block_hash).is_awaiting_precommitments());

        // Getting anther block proposal from the same peer should not change the state
        non_coords[0]
            .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
            .await
            .expect("valid block proposal");
        assert_eq!(non_coords[0].pre_commitments.read().await.get(&block_hash).unwrap().len(), 2);
        assert!(non_coords[0].get_state(block_hash).is_awaiting_precommitments());

        // Test that the other non-coord responds the same way
        non_coords[1]
            .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
            .await
            .expect("valid block proposal");
        // There should be two commitments
        assert_eq!(non_coords[1].pre_commitments.read().await.get(&block_hash).unwrap().len(), 2);
        assert!(non_coords[1].get_state(block_hash).is_awaiting_precommitments());
    }

    #[ignore]
    #[tokio::test]
    async fn pre_commitments_flow() {
        // Note: set up test signs with the first authorities key
        setup_multi_party_test!(
            3,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            block_to_propose,
            mock_eth_provider,
            mock_network_client
        );

        // Propose valid block and assert correct state transitions
        let block_hash = block_to_propose
            .header()
            .segregated_signature_block_hash()
            .expect("to get the block hash");

        non_coords[0]
            .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
            .await
            .expect("valid block proposal");
        // There should be two commitments
        assert_eq!(non_coords[0].pre_commitments.read().await.get(&block_hash).unwrap().len(), 2);
        assert!(non_coords[0].get_state(block_hash).is_awaiting_precommitments());

        // Getting anther block proposal from the same peer should not change the state
        non_coords[0]
            .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
            .await
            .expect("valid block proposal");
        assert_eq!(non_coords[0].pre_commitments.read().await.get(&block_hash).unwrap().len(), 2);
        assert!(non_coords[0].get_state(block_hash).is_awaiting_precommitments());

        let other_peer_id = non_coords[1].peer_id.clone();
        // Process other peers pre-commitment
        non_coords[0]
            .process_precommitment(block_to_propose.clone(), other_peer_id)
            .await
            .expect("valid precommitment");

        // There should be three pre-commitments, non_coord[0], coord which was added at the block
        // proposal stage And non_coord[1] which we just added
        let pre_commitments =
            non_coords[0].pre_commitments.read().await.get(&block_hash).cloned().unwrap();
        assert_eq!(pre_commitments.len(), 3);
        for i in 0..pre_commitments.len() {
            assert!(pre_commitments.contains(&peer_ids[i]));
        }
        assert!(non_coords[0].get_state(block_hash).is_awaiting_commitments());

        // Adding the same pre-commit from the same peer shouldnt change anything b/c we are await
        // for commitments
        non_coords[0]
            .process_precommitment(block_to_propose.clone(), other_peer_id)
            .await
            .expect("valid precommitment");
        let mut pre_commitments =
            non_coords[0].pre_commitments.read().await.get(&block_hash).unwrap().clone();
        assert_eq!(pre_commitments.len(), 3);
        for i in 0..pre_commitments.len() {
            assert!(pre_commitments.contains(&peer_ids[i]));
        }
        assert!(non_coords[0].get_state(block_hash).is_awaiting_commitments());

        // Remove the coord's pre-commits
        pre_commitments.remove(&coord.peer_id);
        non_coords[0].pre_commitments.write().await.insert(block_hash, pre_commitments.clone());
        non_coords[0].set_state(PbftState::AwaitingPreCommitments, block_hash);
        // Adding the same pre-commit here is requesting another signed block. This will fail b/c we
        // have already signed for this timeslot
        let res =
            non_coords[0].process_precommitment(block_to_propose.clone(), other_peer_id).await;
        assert!(res.err().unwrap().to_string().contains("Peer for time slot"));

        let pre_commitments =
            non_coords[0].pre_commitments.read().await.get(&block_hash).unwrap().clone();
        assert_eq!(pre_commitments.len(), 2);
        // At this point its us (non_coord[0]) and the other peer (non_coord[1)
        assert!(pre_commitments.contains(&non_coords[0].peer_id));
        assert!(pre_commitments.contains(&non_coords[1].peer_id));
        assert!(non_coords[0].get_state(block_hash).is_awaiting_precommitments());
    }

    #[ignore]
    #[tokio::test]
    async fn commitments_flow() {
        // Note: set up test signs with the first authorities key
        setup_multi_party_test!(
            3,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            block_to_propose,
            mock_eth_provider,
            mock_network_client
        );

        coord.init_block_proposal(block_to_propose.clone()).await.expect("valid block proposal");

        // Process block proposal
        let block_hash = block_to_propose
            .header()
            .segregated_signature_block_hash()
            .expect("to get the block hash");
        for i in 0..non_coords.len() {
            non_coords[i]
                .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
                .await
                .expect("valid block proposal");
        }
        // At this point we should have two pre-commitments
        // The other non-coord peers need to provide their pre-commitments

        let peer_id_0 = non_coords[0].peer_id.clone();
        let peer_id_1 = non_coords[1].peer_id.clone();
        // Process other peers pre-commitment
        non_coords[0]
            .process_precommitment(block_to_propose.clone(), peer_id_1)
            .await
            .expect("valid precommitment");

        non_coords[1]
            .process_precommitment(block_to_propose.clone(), peer_id_0)
            .await
            .expect("valid precommitment");

        // both non-coords should be awaiting commitments
        assert!(non_coords[0].get_state(block_hash).is_awaiting_commitments());
        assert!(non_coords[1].get_state(block_hash).is_awaiting_commitments());
        // Coordinator still havent received enough commitments
        assert!(coord.get_state(block_hash).is_awaiting_precommitments());

        coord
            .process_precommitment(block_to_propose.clone(), peer_id_0)
            .await
            .expect("valid precommitment");

        coord
            .process_precommitment(block_to_propose.clone(), peer_id_1)
            .await
            .expect("valid precommitment");

        // Coordinator should now be awaiting commitments
        assert!(coord.get_state(block_hash).is_awaiting_commitments());

        // Sign the block as peer 1
        let mut header_to_sign_0 = block_to_propose.header().clone();
        header_to_sign_0.sign_block(&non_coords[0].secret_key).expect("to sign block");
        let signed_block_0 = SealedBlock::new(header_to_sign_0.seal_slow(), BlockBody::default());
        assert_eq!(signed_block_0.header().segregated_signature_block_hash().unwrap(), block_hash);

        // Sign the block as peer 2
        let mut header_to_sign_1 = block_to_propose.header().clone();
        header_to_sign_1.sign_block(&non_coords[1].secret_key).expect("to sign block");
        let signed_block_1 = SealedBlock::new(header_to_sign_1.seal_slow(), BlockBody::default());
        assert_eq!(signed_block_1.header().segregated_signature_block_hash().unwrap(), block_hash);

        coord
            .process_commitment(signed_block_0.clone(), peer_id_0)
            .await
            .expect("valid commitment");
        // Coordinator should still be awaiting commitments
        assert!(coord.get_state(block_hash).is_awaiting_commitments());
        let sigs_so_far = coord
            .sealed_blocks
            .read()
            .await
            .get(&block_hash)
            .unwrap()
            .header()
            .clone()
            .deserialize_extra_data_header()
            .unwrap()
            .authority_signatures
            .expect("should have signatures");
        // Coord has its own signature and the one from peer 0
        assert_eq!(sigs_so_far.len(), 2);

        // adding the same commitment should not change anything
        coord
            .process_commitment(signed_block_0.clone(), peer_id_0)
            .await
            .expect("valid commitment");
        let sigs_again = coord
            .sealed_blocks
            .read()
            .await
            .get(&block_hash)
            .unwrap()
            .header()
            .clone()
            .deserialize_extra_data_header()
            .unwrap()
            .authority_signatures
            .expect("should have signatures");
        assert_eq!(sigs_again.len(), 2);
        assert!(sigs_again.contains(&sigs_so_far[0]));
        assert!(sigs_again.contains(&sigs_so_far[1]));

        let finished_block = coord
            .process_commitment(signed_block_1.clone(), peer_id_1)
            .await
            .expect("valid commitment");
        // Since we added the last signature needed we should get returned `Some(SealedBlock)`
        assert!(finished_block.is_some());

        let sigs_so_far = coord
            .sealed_blocks
            .read()
            .await
            .get(&block_hash)
            .unwrap()
            .header()
            .clone()
            .deserialize_extra_data_header()
            .unwrap()
            .authority_signatures
            .expect("should have signatures");
        // There should be a sig from all peers
        assert_eq!(sigs_so_far.len(), 3);
        for sk in sks {
            let pk = sk.public_key(secp256k1::SECP256K1);
            let mut recovered = false;
            for sig in sigs_so_far.iter() {
                let msg = secp256k1::Message::from_digest_slice(
                    &block_to_propose.header().create_sighash().unwrap().0.as_slice(),
                )
                .unwrap();
                let recovered_pk = sig.recover(&msg).unwrap();
                if recovered_pk == pk {
                    recovered = true;
                    break;
                }
            }

            assert!(recovered);
        }
    }

    #[ignore]
    #[tokio::test]
    async fn cannot_suggest_the_same_block_twice() {
        // Note: set up test signs with the first authorities key
        setup_multi_party_test!(
            3,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            block_to_propose,
            mock_eth_provider,
            mock_network_client
        );

        coord.init_block_proposal(block_to_propose.clone()).await.expect("valid block proposal");
        // Process block proposal
        let block_hash = block_to_propose
            .header()
            .segregated_signature_block_hash()
            .expect("to get the block hash");
        for i in 0..non_coords.len() {
            non_coords[i]
                .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
                .await
                .expect("valid block proposal");
        }
        // At this point we should have two pre-commitments
        // The other non-coord peers need to provide their pre-commitments

        let peer_id_0 = non_coords[0].peer_id.clone();
        let peer_id_1 = non_coords[1].peer_id.clone();
        // Process other peers pre-commitment
        non_coords[0]
            .process_precommitment(block_to_propose.clone(), peer_id_1)
            .await
            .expect("valid precommitment");

        non_coords[1]
            .process_precommitment(block_to_propose.clone(), peer_id_0)
            .await
            .expect("valid precommitment");

        coord
            .process_precommitment(block_to_propose.clone(), peer_id_0)
            .await
            .expect("valid precommitment");

        coord
            .process_precommitment(block_to_propose.clone(), peer_id_1)
            .await
            .expect("valid precommitment");

        // Coordinator should now be awaiting commitments
        assert!(coord.get_state(block_hash).is_awaiting_commitments());
        assert!(non_coords[0].get_state(block_hash).is_awaiting_commitments());
        assert!(non_coords[1].get_state(block_hash).is_awaiting_commitments());

        // Sign block as peer 0
        let mut header_to_sign_0 = block_to_propose.header().clone();
        header_to_sign_0.sign_block(&non_coords[0].secret_key).expect("to sign block");
        let signed_block_0 = SealedBlock::new(header_to_sign_0.seal_slow(), BlockBody::default());

        non_coords[0]
            .process_commitment(signed_block_0, peer_ids[0])
            .await
            .expect("valid commitment");
    }

    /* Validating fork */
    #[tokio::test]
    async fn will_not_sign_if_block_is_known() {
        setup_multi_party_test!(
            1,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            block_to_propose,
            mock_eth_provider,
            mock_network_client
        );

        // frost config is not needed for this test
        let pbft_state_machine = coord;
        mock_eth_provider.add_block(block_to_propose.hash(), block_to_propose.clone().into());

        let res = pbft_state_machine.validate_block(&block_to_propose).await;

        assert_eq!(
            res.err().unwrap(),
            ValidateBlockError::BlockAlreadyInCanonChain(block_to_propose.hash_slow())
        );
    }

    #[tokio::test]
    async fn signing_on_parent_block() {
        setup_multi_party_test!(
            1,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            _block_to_propose,
            mock_eth_provider,
            mock_network_client
        );
        let mock_eth_provider = MockEthProvider::default();
        // frost config is not needed for this test
        let sk = coord.secret_key.clone();
        let pbft_state_machine = coord;
        let edh = ExtraDataHeader::default();
        let mut parent_header = Header::default();
        parent_header.add_extra_data_header(&edh);
        parent_header.sign_block(&sk).expect("to sign block");
        let parent_block = SealedBlock::new(parent_header.seal_slow(), BlockBody::default());
        mock_eth_provider.add_block(parent_block.hash(), parent_block.clone().into());

        let mut header = Header::default();
        header.add_extra_data_header(&edh);
        header.number = 1;
        header.parent_hash = parent_block.hash_slow();
        header.sign_block(&sk).expect("to sign block");
        let block = SealedBlock::new(header.seal_slow(), BlockBody::default());

        let res = pbft_state_machine.validate_block(&block).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn signing_genisis_block() {
        let mock_eth_provider = MockEthProvider::default();
        // frost config is not needed for this test
        let secp = secp256k1::Secp256k1::new();
        let sk = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let config = FrostConfig {
            authorities: vec![],
            authority_index: 0,
            max_signers: 0,
            min_signers: 0,
            authority_pk: sk.public_key(SECP256K1),
        };
        let mock_network_client = MockNetworkClient::new(mock_eth_provider.clone());
        let storage = StoragePBFT::new(
            vec![],
            vec![],
            Some(secp256k1::PublicKey::from_secret_key(&secp, &sk)),
            bitcoin::Network::from_core_arg("regtest").expect("regtest exists"),
        );
        let pbft_state_machine = PbftStateMachine::new(
            mock_eth_provider.clone(),
            storage,
            FrostHandleMock {},
            config,
            PeerId::default(),
            sk.clone(),
            None,
            mock_network_client,
            Arc::new(RwLock::new(None)),
            TestExecutorFactory::default(),
            AuthorityConsensus::new(Arc::clone(&mock_eth_provider.chain_spec)),
        );

        let edh = ExtraDataHeader::default();
        let mut header = Header::default();
        header.add_extra_data_header(&edh);
        header.number = 0;
        header.parent_hash = B256::ZERO;
        header.sign_block(&sk).expect("to sign block");
        let block = SealedBlock::new(header.seal_slow(), BlockBody::default());

        let res = pbft_state_machine.validate_block(&block).await;

        assert_eq!(res.err().unwrap(), ValidateBlockError::WillNotSignGenesisBlock);
    }

    #[tokio::test]
    async fn will_not_sign_if_parent_block_is_known_but_not_canon() {
        setup_multi_party_test!(
            1,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            _block_to_propose,
            mock_eth_provider,
            mock_network_client
        );

        // frost config is not needed for this test
        let sk = coord.secret_key.clone();
        let pbft_state_machine = coord;
        let edh = ExtraDataHeader::default();
        let mut b0 = Header::default();
        b0.add_extra_data_header(&edh);
        b0.sign_block(&sk).expect("to sign block");
        let b0_block = SealedBlock::new(b0.clone().seal_slow(), BlockBody::default());
        mock_eth_provider.add_block(b0_block.hash(), b0_block.clone().into());

        let mut b1 = Header::default();
        b1.add_extra_data_header(&edh);
        b1.number = 1;
        b1.parent_hash = b0_block.hash_slow();
        b1.sign_block(&sk).expect("to sign block");
        let parent_block = SealedBlock::new(b1.seal_slow(), BlockBody::default());
        mock_eth_provider.add_block(parent_block.hash(), parent_block.clone().into());

        let mut header = Header::default();
        header.add_extra_data_header(&edh);
        header.number = 2;
        header.parent_hash = b0.clone().hash_slow();
        header.sign_block(&sk).expect("to sign block");
        let block = SealedBlock::new(header.seal_slow(), BlockBody::default());

        let res = pbft_state_machine.validate_block(&block).await;
        assert_eq!(
            res.err().unwrap(),
            ValidateBlockError::ParentHashNotCanonicalTip(block.hash_slow(), b0.hash_slow())
        );
    }

    #[tokio::test]
    async fn will_sign_for_valid_fork() {
        // Calling the macro just to get the coord sk
        setup_multi_party_test!(
            1,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            _block_to_propose,
            mock_eth_provider,
            mock_network_client
        );
        // to simulate the fork we will create two providers
        // one for our and another which the network client will use to "fetch"
        // blocks from the peer
        let mock_eth_provider_mine = MockEthProvider::default();
        let mock_eth_provider_peers = MockEthProvider::default();
        // frost config is not needed for this test
        let secp = secp256k1::Secp256k1::new();
        let sk = coord.secret_key.clone();
        let config = coord.config.clone();
        let mock_network_client_peers = MockNetworkClient::new(mock_eth_provider_peers.clone());

        let storage = StoragePBFT::new(
            vec![],
            vec![],
            Some(secp256k1::PublicKey::from_secret_key(&secp, &sk)),
            bitcoin::Network::from_core_arg("regtest").expect("regtest exists"),
        );
        let pbft_state_machine = PbftStateMachine::new(
            mock_eth_provider_mine.clone(),
            storage,
            FrostHandleMock {},
            config,
            PeerId::default(),
            sk.clone(),
            None,
            mock_network_client_peers,
            Arc::new(RwLock::new(None)),
            TestExecutorFactory::default(),
            AuthorityConsensus::new(Arc::clone(&mock_eth_provider.chain_spec)),
        );
        let edh = ExtraDataHeader::default();
        let mut b0 = Header::default();
        b0.add_extra_data_header(&edh);
        b0.sign_block(&sk).expect("to sign block");
        let b0_block = SealedBlock::new(b0.clone().seal_slow(), BlockBody::default());
        mock_eth_provider_mine.add_block(b0_block.hash(), b0_block.clone().into());

        // Now we create a fork
        // Something has to be different btwn the blocks, so we can modify the nonce
        let mut b1_0 = Header::default();
        b1_0.add_extra_data_header(&edh);
        b1_0.number = 1;
        b1_0.parent_hash = b0_block.hash_slow();
        b1_0.nonce = 0;
        b1_0.sign_block(&sk).expect("to sign block");
        let parent_block = SealedBlock::new(b1_0.seal_slow(), BlockBody::default());
        mock_eth_provider_mine.add_block(parent_block.hash(), parent_block.clone().into());

        // we'll propose a block on top of the fork
        let mut b1_1 = Header::default();
        b1_1.add_extra_data_header(&edh);
        b1_1.number = 1;
        b1_1.parent_hash = b0_block.hash_slow();
        b1_1.nonce = 2;
        b1_1.sign_block(&sk).expect("to sign block");
        let parent_block = SealedBlock::new(b1_1.clone().seal_slow(), BlockBody::default());
        mock_eth_provider_peers.add_block(parent_block.hash(), parent_block.clone().into());

        let mut header = Header::default();
        header.add_extra_data_header(&edh);
        header.number = 2;
        header.parent_hash = b1_1.clone().hash_slow();
        header.sign_block(&sk).expect("to sign block");
        let block = SealedBlock::new(header.seal_slow(), BlockBody::default());

        let res = pbft_state_machine.validate_block(&block).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn will_not_sign_for_forks_deeper_than_1() {
        // Calling the macro just to get the coord sk
        setup_multi_party_test!(
            1,
            sks,
            frost_handle_mock,
            configs,
            peer_ids,
            signed_blocks,
            non_coords,
            coord,
            _block_to_propose,
            mock_eth_provider,
            mock_network_client
        );
        // to simulate the fork we will create two providers
        // one for our and another which the network client will use to "fetch"
        // blocks from the peer
        let mock_eth_provider_mine = MockEthProvider::default();
        let mock_eth_provider_peers = MockEthProvider::default();
        // frost config is not needed for this test
        let secp = secp256k1::Secp256k1::new();
        let sk = coord.secret_key.clone();
        let config = coord.config.clone();
        let mock_network_client_peers = MockNetworkClient::new(mock_eth_provider_peers.clone());

        let storage = StoragePBFT::new(
            vec![],
            vec![],
            Some(secp256k1::PublicKey::from_secret_key(&secp, &sk)),
            bitcoin::Network::from_core_arg("regtest").expect("regtest exists"),
        );
        let pbft_state_machine = PbftStateMachine::new(
            mock_eth_provider_mine.clone(),
            storage,
            FrostHandleMock {},
            config,
            PeerId::default(),
            sk.clone(),
            None,
            mock_network_client_peers,
            Arc::new(RwLock::new(None)),
            TestExecutorFactory::default(),
            AuthorityConsensus::new(Arc::clone(&mock_eth_provider.chain_spec)),
        );
        let edh = ExtraDataHeader::default();
        let mut b0 = Header::default();
        b0.add_extra_data_header(&edh);
        b0.sign_block(&sk).expect("to sign block");
        let b0_block = SealedBlock::new(b0.clone().seal_slow(), BlockBody::default());
        mock_eth_provider_mine.add_block(b0_block.hash(), b0_block.clone().into());

        // Now we create a fork
        // Something has to be different btwn the blocks, so we can modify the nonce
        let mut b1_0 = Header::default();
        b1_0.add_extra_data_header(&edh);
        b1_0.number = 1;
        b1_0.parent_hash = b0_block.hash_slow();
        b1_0.nonce = 0;
        b1_0.sign_block(&sk).expect("to sign block");
        let parent_block = SealedBlock::new(b1_0.seal_slow(), BlockBody::default());
        mock_eth_provider_mine.add_block(parent_block.hash(), parent_block.clone().into());

        // First block of the fork
        let mut b1_1 = Header::default();
        b1_1.add_extra_data_header(&edh);
        b1_1.number = 1;
        b1_1.parent_hash = b0_block.hash_slow();
        b1_1.nonce = 2;
        b1_1.sign_block(&sk).expect("to sign block");
        let parent_block = SealedBlock::new(b1_1.clone().seal_slow(), BlockBody::default());
        mock_eth_provider_peers.add_block(parent_block.hash(), parent_block.clone().into());

        // Second block of the fork
        let mut b2_1 = Header::default();
        b2_1.add_extra_data_header(&edh);
        b2_1.number = 2;
        b2_1.parent_hash = b1_1.hash_slow();
        b2_1.nonce = 3;
        b2_1.sign_block(&sk).expect("to sign block");
        let parent_block = SealedBlock::new(b2_1.clone().seal_slow(), BlockBody::default());
        mock_eth_provider_peers.add_block(parent_block.hash(), parent_block.clone().into());

        let mut header = Header::default();
        header.add_extra_data_header(&edh);
        header.number = 3;
        header.parent_hash = b2_1.clone().hash_slow();
        header.sign_block(&sk).expect("to sign block");
        let block = SealedBlock::new(header.seal_slow(), BlockBody::default());

        let res = pbft_state_machine.validate_block(&block).await;
        assert_eq!(
            res.err().unwrap(),
            ValidateBlockError::ForkDepthGreaterThanOne(block.hash_slow())
        );
    }
}
