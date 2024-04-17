use crate::{utils::retry_exec, Storage};
use frost_secp256k1_tr as frost;
use reth_botanix_lib::extra_data_header::ExtraDataHeaderSerializeError;
use reth_botanix_lib::header_ext::HeaderExt;
use reth_consensus_common::utils::current_inturn_index;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_network::frost::{
    manager::{peer_id_to_identifier, FrostCommand, FrostConfig, FrostHandle},
    FrostPeerCommand, PbftEventResponseType, PbftResponse, PeerMessageResponse,
};
use reth_primitives::{BlockBody, BlockHash, SealedBlock};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use reth_rpc_types::PeerId;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::Duration,
};
use tokio::sync::mpsc::{error::SendError, UnboundedSender};
use tracing::{error, info, warn};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Failed to deserialize extra data header: {0}")]
    ExtraDataHeaderSerializeError(#[from] ExtraDataHeaderSerializeError),
    #[error("Failed to get connected peers handles")]
    FailedToGetConnectedPeersHandles,
    #[error("Failed to send peer command {0}")]
    Send(SendError<FrostPeerCommand>),
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
pub(crate) struct PbftStateMachine<Client> {
    storage: Storage<Client>,
    frost_handle: FrostHandle,
    state: PbftState,
    /// our peer id
    peer_id: PeerId,
    config: FrostConfig,
    pre_commitments: BTreeMap<BlockHash, HashSet<PeerId>>,
    commitments: BTreeMap<BlockHash, HashSet<PeerId>>,
    secret_key: secp256k1::SecretKey,
    personal_frost_identifier: frost::Identifier,
}

impl<Client> PbftStateMachine<Client>
where
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
{
    /// Constructs a new state machine with the given params
    pub(crate) fn new(
        storage: Storage<Client>,
        frost_handle: FrostHandle,
        config: FrostConfig,
        peer_id: PeerId,
        secret_key: secp256k1::SecretKey,
    ) -> Self {
        let personal_frost_identifier: frost::Identifier =
            peer_id_to_identifier(config.authority_index as u16);
        info!(
            "Frost identifier used: {:?} - {:?}",
            config.authority_index, personal_frost_identifier
        );
        Self {
            personal_frost_identifier,
            storage,
            frost_handle,
            state: PbftState::Initial,
            config,
            peer_id,
            pre_commitments: BTreeMap::new(),
            commitments: BTreeMap::new(),
            secret_key,
        }
    }

    /// Resets the state machine to its initial state
    #[allow(dead_code)]
    pub(crate) fn reset(self) -> Self {
        Self {
            personal_frost_identifier: self.personal_frost_identifier,
            storage: self.storage,
            frost_handle: self.frost_handle,
            state: PbftState::Initial,
            config: self.config,
            peer_id: self.peer_id,
            pre_commitments: BTreeMap::new(),
            commitments: BTreeMap::new(),
            secret_key: self.secret_key,
        }
    }

    /// Returns the state machine state
    pub(crate) fn get_state(&self) -> PbftState {
        self.state
    }

    /// Sets state machine state
    pub(crate) fn set_state(&mut self, state: PbftState) {
        self.state = state;
    }
}

impl<Client> PbftStateMachine<Client>
where
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
{
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
                error!("Failed to get frost peers connections {:?}", e);
                return Err(Error::FailedToGetConnectedPeersHandles);
            }
        }
    }

    pub(crate) async fn gossip_to_peers(
        &mut self,
        pbft_response: PbftResponse,
    ) -> Result<(), Error> {
        let fut = || async {
            // get all connected peers
            let connected_peers = self.get_all_peers_handle().await?;

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

    pub(crate) async fn process_block_proposal(
        &mut self,
        block: SealedBlock,
        peer_id: PeerId,
    ) -> Result<(), Error> {
        info!(target: "pbft" ,"Processing block proposal from peer {:?}", peer_id);
        let coordinator = self
            .config
            .authorities
            .get(current_inturn_index(self.config.authorities.len() as u64) as usize)
            .expect("should be valid index");

        // TODO Check if the inturn block producer has a signature on the block
        let block_hash = block.hash_slow();
        self.pre_commitments.insert(block_hash, HashSet::new());
        self.process_precommitment(block, peer_id).await?;
        Ok(())
    }

    pub(crate) async fn process_precommitment(
        &mut self,
        block: SealedBlock,
        peer_id: PeerId,
    ) -> Result<(), Error> {
        info!(target: "pbft" ,"Processing pre-commitment from peer {:?}", peer_id);
        let block_hash = block.hash_slow();
        // This shouldnt be the first time we are seeing this block
        let pre_commits = self.pre_commitments.get(&block_hash).unwrap_or(&HashSet::new()).clone();
        let mut pre_commits_mut = pre_commits.clone();
        // Add Peers precommitment
        pre_commits_mut.insert(peer_id);
        // Add our own precommitment
        pre_commits_mut.insert(self.peer_id);
        self.pre_commitments.insert(block_hash, pre_commits_mut);

        // if we have enough precommitments, we can move to the next state
        if pre_commits.len() >= self.config.min_signers as usize {
            info!(target: "pbft" ,"We have enough pre-commitments moving to next state");
            let mut mutable_header = block.header().clone();
            mutable_header.sign_block(&self.secret_key).unwrap();
            let signed_block = SealedBlock::new(
                mutable_header.seal_slow(),
                BlockBody { transactions: block.body, ommers: vec![], withdrawals: None },
            );

            let commitment = PbftResponse {
                response_type: PbftEventResponseType::PeerCommitment,
                data: signed_block,
            };
            self.commitments.insert(block_hash, HashSet::new());
            self.commitments.get_mut(&block_hash).unwrap().insert(self.peer_id);

            self.gossip_to_peers(commitment).await?;
        } else {
            // Generate our pre commitment gossip it out
            let precommit = PbftResponse {
                response_type: PbftEventResponseType::PeerPreCommitment,
                data: block,
            };
            self.gossip_to_peers(precommit).await?;
        }
        Ok(())
    }

    pub(crate) async fn process_commitment(
        &mut self,
        block: SealedBlock,
        peer_id: PeerId,
    ) -> Result<(), Error> {
        let block_hash = block.hash_slow();
        let mut commits = self.commitments.get(&block_hash).unwrap_or(&HashSet::new()).clone();
        commits.insert(peer_id);

        // if we have enough commitments, we can move to the next state
        if commits.len() >= self.config.min_signers as usize {
            info!(target: "pbft" ,"We have enough commitments moving to next state");
            self.commitments.remove(&block_hash);
            // TODO: we should be able to move to the next state
        }
        Ok(())
    }
}
