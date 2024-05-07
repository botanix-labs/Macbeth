use crate::utils::retry_exec;
use reth_consensus_common::utils::is_inturn;
use reth_network::frost::manager::ToFrostManager;

use frost_secp256k1_tr as frost;
use reth_botanix_lib::extra_data_header::{
    ExtraDataHeaderDeserialzeError, ExtraDataHeaderSerializeError, ValidateAuthoritySignatureError,
};
use reth_botanix_lib::header_ext::HeaderExt;
use reth_consensus_common::utils::current_inturn_index;
use reth_ecies::util::pk2id;
use reth_network::frost::{
    manager::{peer_id_to_identifier, FrostCommand, FrostConfig, FrostHandle},
    FrostPeerCommand, PbftEventResponseType, PbftResponse, PeerMessageResponse,
};
use reth_primitives::{BlockBody, BlockHash, SealedBlock};
use reth_rpc_types::PeerId;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::Duration,
};
use tokio::sync::mpsc::{error::SendError, UnboundedSender};
use tracing::{debug, error, info, warn};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Failed to validate signatures on block: {0}")]
    InvalidSignature(#[from] ValidateAuthoritySignatureError),
    #[error("Failed to deserialize extra data header: {0}")]
    ExtraDataHeaderDeserializeError(#[from] ExtraDataHeaderDeserialzeError),
    #[error("Failed to serialize extra data header: {0}")]
    ExtraDataHeaderSerializeError(#[from] ExtraDataHeaderSerializeError),
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
pub(crate) struct PbftStateMachine<F: ToFrostManager> {
    frost_handle: F,
    state: BTreeMap<BlockHash, PbftState>,
    /// our peer id
    peer_id: PeerId,
    config: FrostConfig,
    pre_commitments: BTreeMap<BlockHash, HashSet<PeerId>>,
    sealed_blocks: BTreeMap<BlockHash, SealedBlock>,
    secret_key: secp256k1::SecretKey,
    personal_frost_identifier: frost::Identifier,
}

impl<F: ToFrostManager> PbftStateMachine<F> {
    /// Constructs a new state machine with the given params
    pub(crate) fn new(
        frost_handle: F,
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
            frost_handle,
            state: BTreeMap::new(),
            config,
            peer_id,
            pre_commitments: BTreeMap::new(),
            sealed_blocks: BTreeMap::new(),
            secret_key,
        }
    }

    /// Resets the state machine to its initial state
    pub(crate) fn reset(self) -> Self {
        Self {
            personal_frost_identifier: self.personal_frost_identifier,
            frost_handle: self.frost_handle,
            state: BTreeMap::new(),
            config: self.config,
            peer_id: self.peer_id,
            pre_commitments: BTreeMap::new(),
            sealed_blocks: BTreeMap::new(),
            secret_key: self.secret_key,
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

impl<F: ToFrostManager> PbftStateMachine<F> {
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
            info!(target: "pbft" ,"Broadcasting pbft response to all peers");
            info!(target: "pbft" ,"Connected peers: {:?}", connected_peers.keys().collect::<Vec<_>>() );

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

    /// Intended to be called by the in turn block producer when a block is ready to be
    /// proposed to the network
    /// Note: there should already be a signature on the block at this point
    pub(crate) async fn init_block_proposal(&mut self, block: SealedBlock) -> Result<(), Error> {
        // Check if there is already a running state machine for this block
        let block_hash = block.header.segregated_signature_block_hash()?;
        let current_state = self.get_state(block_hash);

        if current_state.is_running() {
            warn!(target: "pbft" ,"State machine is already running for block {:?}", block_hash);
            return Ok(());
        }

        if !self.is_coordinator() {
            warn!(target: "pbft" ,"Not the coordinator -- ignoring init block proposal request");
            return Ok(());
        }

        if block.header.deserialize_extra_data_header()?.authority_signatures.is_none() {
            warn!(target: "pbft" ,"Block proposal does not contain any signatures");
            return Err(Error::MissingSignatures);
        }

        // Save block locally
        self.sealed_blocks.insert(block_hash, block.clone());
        println!("block hash: {:?}", block_hash);

        // Set the state to awaiting pre-commitments
        self.set_state(PbftState::AwaitingPreCommitments, block_hash);
        self.gossip_to_peers(PbftResponse {
            response_type: PbftEventResponseType::CoordinatorBlockProposal,
            data: block.clone(),
        })
        .await?;

        // As the coordinator we can add our own pre-commitment
        self.pre_commitments.entry(block_hash).or_insert_with(HashSet::new).insert(self.peer_id);

        Ok(())
    }

    /// Block proposal from the in turn block producer
    /// We should only be getting this request from the in turn block producer
    pub(crate) async fn process_block_proposal(
        &mut self,
        block: SealedBlock,
        peer_id: PeerId,
    ) -> Result<(), Error> {
        info!(target: "pbft" ,"Processing block proposal from peer {:?}", peer_id);
        let block_hash = block.header.segregated_signature_block_hash()?;
        let current_state = self.get_state(block_hash);
        if current_state.is_running() {
            warn!(target: "pbft" ,"State machine is already running for block {:?}", block_hash);
            return Ok(());
        }

        if peer_id == self.peer_id {
            return Ok(());
        }

        let coordinator = self
            .config
            .authorities
            .get(current_inturn_index(self.config.authorities.len() as u64) as usize)
            .expect("should be valid index");

        // Check if the inturn block producer has the first signature on the block
        // this serves as authentication that the block was produced by the in turn block producer
        match block.header.deserialize_extra_data_header()?.authority_signatures {
            Some(sigs) => {
                if sigs.len() > 1 {
                    return Err(Error::TooManySignaturesOnProposedBlock);
                }
                let msg =
                    secp256k1::Message::from_slice(&block.header.create_sighash()?.0.as_slice())?;
                let recovered_pk = sigs[0].recover(&msg)?;
                if recovered_pk != *coordinator {
                    warn!(target: "pbft" ,"In turn block producer does not have the first signature on the block");
                    return Err(Error::MissingInTurnSignature);
                }
            }
            None => {
                warn!(target: "pbft" ,"Block proposal does not contain any signatures");
                return Err(Error::MissingSignatures);
            }
        }

        // Add our own pre-commitment
        let mut pre_commits = HashSet::new();
        pre_commits.insert(self.peer_id);
        // And implicitly add the coordinator's pre-commitment
        pre_commits.insert(peer_id);
        self.pre_commitments.insert(block_hash, pre_commits);
        self.set_state(PbftState::AwaitingPreCommitments, block_hash);

        // Broadcast our pre-commitment
        self.gossip_to_peers(PbftResponse {
            response_type: PbftEventResponseType::PeerPreCommitment,
            data: block.clone(),
        })
        .await?;

        // Edge case: In a two person federation we can move to the next state
        self.check_and_send_commitment(&block).await?;

        Ok(())
    }

    /// Check if we have enough pre-commits to move onto the next state
    /// If we do, we can send our commitment
    async fn check_and_send_commitment(&mut self, block: &SealedBlock) -> Result<(), Error> {
        let block_hash = block.header.segregated_signature_block_hash()?;

        let pre_commits =
            self.pre_commitments.get(&block_hash).cloned().unwrap_or_else(HashSet::new);
        // if we have enough precommitments, we can move to the next state
        if pre_commits.len() >= self.config.max_signers as usize {
            info!(target: "pbft" ,"We have enough pre-commitments moving to next state");
            let mut mutable_header = block.header().clone();
            mutable_header.sign_block(&self.secret_key)?;
            let signed_block = SealedBlock::new(
                mutable_header.seal_slow(),
                BlockBody { transactions: block.body.clone(), ommers: vec![], withdrawals: None },
            );

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
        info!(target: "pbft", "Processing pre-commitment from peer {:?}", peer_id);
        let block_hash = block.header.segregated_signature_block_hash()?;
        let current_state = self.get_state(block_hash);
        if !current_state.is_awaiting_precommitments() {
            warn!(target: "pbft", "State machine is not awaiting pre-commitments for block {:?}", block_hash);
            return Ok(());
        }

        // Do not process our own response
        if peer_id == self.peer_id {
            return Ok(());
        }

        // Add the peer's precommitment
        let pre_commits = self.pre_commitments.entry(block_hash).or_insert_with(HashSet::new);
        pre_commits.insert(peer_id);
        info!(target: "pbft" ,"pre-commitments: {:?}", pre_commits.len());

        self.check_and_send_commitment(&block).await?;

        Ok(())
    }

    /// Process a commitment from a peer
    /// If we have enough commitments, returns true
    /// Otherwise returns false
    pub(crate) async fn process_commitment(
        &mut self,
        block: SealedBlock,
        peer_id: PeerId,
    ) -> Result<Option<SealedBlock>, Error> {
        // Only the in turn coordinator should be processing commitments
        if !self.is_coordinator() {
            warn!(target: "pbft" ,"Not the coordinator -- ignoring commitment from peer {:?}", peer_id);
            return Ok(None);
        }
        if peer_id == self.peer_id {
            return Ok(None);
        }
        let block_hash = block.header.segregated_signature_block_hash()?;
        // Check that this peer specifically provided a signature
        let current_state = self.get_state(block_hash);
        if !current_state.is_awaiting_commitments() {
            warn!(target: "pbft" ,"State machine is not awaiting commitments for block {:?}", block_hash);
            return Ok(None);
        }

        // This block is originally added during init block proposal
        let mut current_header =
            self.sealed_blocks.get(&block_hash).expect("block should exist").header().clone();
        let mut edh = current_header.deserialize_extra_data_header()?;
        let peer_edh = block.header().deserialize_extra_data_header()?;

        if peer_edh.authority_signatures.is_none() {
            debug!(target: "pbft" ,"Peer did not provide a signature");
            return Ok(None);
        }

        // Check that the commited block is the same as the block we are tracking
        if current_header.segregated_signature_block_hash()?
            != block.header.segregated_signature_block_hash()?
        {
            warn!(target: "pbft" ,"Block hash recieved from peer does not match the block we are tracking");
            return Ok(None);
        }
        // Check all the signatures on the commited block from the peer
        peer_edh.check_authority_sig_add(
            &current_header.create_sighash()?.to_vec(),
            &self.config.authorities,
        )?;

        // Should merge this peers siganture into the main block where we are tracking all signatures
        // If that signature provided is not valid fail
        // If they did not provide a sig fail
        // merge signature from peer
        edh.merge_signature(&peer_edh);
        // update header
        current_header.add_extra_data_header(&edh);
        let new_block = SealedBlock::new(
            current_header.clone().seal_slow(),
            BlockBody { transactions: block.body.clone(), ommers: vec![], withdrawals: None },
        );
        // Update local state
        self.sealed_blocks.insert(block_hash, new_block.clone());
        let number_of_valid_sigs = edh.check_authority_sig_add(
            &current_header.create_sighash()?.to_vec(),
            &self.config.authorities,
        )?;
        println!("number of valid sigs: {}", number_of_valid_sigs);
        println!("max signers: {}", self.config.max_signers);
        // if we have enough commitments, we can move to the next state
        if number_of_valid_sigs >= self.config.max_signers {
            info!(target: "pbft" ,"We have enough commitments, time to produce a block");
            // TODO remove debug
            let sigs = edh.authority_signatures.unwrap();
            info!(target: "pbft" ,"signatures: {:?}", sigs);
            return Ok(Some(new_block));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    macro_rules! setup_multi_party_test {
        ($n:expr, $sks:ident, $frost_handle_mock:ident, $configs:ident, $peer_ids:ident, $signed_blocks:ident, $non_coords:ident, $coord:ident, $block_to_propose:ident) => {
            let secp = secp256k1::Secp256k1::new();
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

            for i in 0..$n {
                let edh = ExtraDataHeader::default();
                let mut header = Header::default();
                header.add_extra_data_header(&edh);
                header.sign_block(&$sks[i]).unwrap();
                let block_body = BlockBody::default();
                $signed_blocks.push(SealedBlock::new(header.seal_slow(), block_body));
            }

            let mut $non_coords = vec![];
            let mut $block_to_propose = None;
            let mut $coord = None;

            for i in 0..$n {
                let pbft_state_machine = PbftStateMachine::new(
                    $frost_handle_mock.clone(),
                    $configs[i].clone(),
                    $peer_ids[i],
                    $sks[i],
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

    use super::*;
    use rand;
    use reth_botanix_lib::extra_data_header::ExtraDataHeader;
    use reth_network::frost::manager::ToFrostManager;
    use reth_primitives::{Block, Header};

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
            _block_to_propose
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
            block_to_propose
        );
        let pbft_state_machine = coord;
        let block_hash = block_to_propose
            .header()
            .segregated_signature_block_hash()
            .expect("to get the block hash");
        // if the state is not init for this block hash it should fail
        // pbft_state_machine.set_state(PbftState::AwaitingCommitments, block_hash);
        // pbft_state_machine.init_block_proposal(block_to_propose.clone()).await.expect("valid block proposal");

        // reset and this time the state should be waiting for pre-commitments
        let mut pbft_state_machine = pbft_state_machine.reset();
        pbft_state_machine
            .init_block_proposal(block_to_propose.clone())
            .await
            .expect("valid block proposal");
        assert_eq!(pbft_state_machine.sealed_blocks.get(&block_hash).unwrap(), &block_to_propose);
        // there should only be the one pre-commitment
        assert_eq!(pbft_state_machine.pre_commitments.get(&block_hash).unwrap().len(), 1);
        assert_eq!(pbft_state_machine.get_state(block_hash), PbftState::AwaitingPreCommitments);

        // Since we are now waiting for pre commitments it should not change the state
        pbft_state_machine
            .init_block_proposal(block_to_propose.clone())
            .await
            .expect("valid block proposal");
        assert_eq!(pbft_state_machine.sealed_blocks.get(&block_hash).unwrap(), &block_to_propose);
        // there should only be the one pre-commitment
        assert_eq!(pbft_state_machine.pre_commitments.get(&block_hash).unwrap().len(), 1);
        assert_eq!(pbft_state_machine.get_state(block_hash), PbftState::AwaitingPreCommitments);

        // Re initialing with the same block proposal should not change the state
        pbft_state_machine.set_state(PbftState::Initial, block_hash);
        pbft_state_machine
            .init_block_proposal(block_to_propose.clone())
            .await
            .expect("valid block proposal");
        assert_eq!(pbft_state_machine.sealed_blocks.get(&block_hash).unwrap(), &block_to_propose);
        // there should only be the one pre-commitment
        assert_eq!(pbft_state_machine.pre_commitments.get(&block_hash).unwrap().len(), 1);
        assert_eq!(pbft_state_machine.get_state(block_hash), PbftState::AwaitingPreCommitments);
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
            block_to_propose
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
        assert_eq!(pbft_state_machine.sealed_blocks.get(&block_hash), None);
        assert_eq!(pbft_state_machine.pre_commitments.get(&block_hash), None);
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
            _block_to_propose
        );

        // sign the block as the non-coordinator
        let non_coord_sk = non_coords[0].secret_key.clone();
        let edh = ExtraDataHeader::default();
        let mut invalid_block_header = Header::default();
        invalid_block_header.add_extra_data_header(&edh);
        invalid_block_header.sign_block(&non_coord_sk).expect("to sign block");
        let invalid_block =
            SealedBlock::new(invalid_block_header.seal_slow(), BlockBody::default());
        // try to propose an a block singed by a non coord
        let res = non_coords[0]
            .process_block_proposal(invalid_block.clone(), coord.peer_id.clone())
            .await;
        assert!(res.is_err());
        assert_eq!(res.err().unwrap().to_string(), "Missing in turn signature on block");
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
            block_to_propose
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
            block_to_propose
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
        assert_eq!(non_coords[0].pre_commitments.get(&block_hash).unwrap().len(), 2);
        // One for the peer that sent their pre commitment
        assert!(non_coords[0].pre_commitments.get(&block_hash).unwrap().contains(&peer_ids[1]));
        // Another implicitly added for the coord that proposed the block
        assert!(non_coords[0].pre_commitments.get(&block_hash).unwrap().contains(&peer_ids[0]));
        // at this point we have two commitments from all peers we should be awaiting commitments
        assert!(non_coords[0].get_state(block_hash).is_awaiting_commitments());

        // Getting anther block proposal from the same peer should not change the state
        non_coords[0]
            .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
            .await
            .expect("valid block proposal");
        assert_eq!(non_coords[0].pre_commitments.get(&block_hash).unwrap().len(), 2);
        assert!(non_coords[0].pre_commitments.get(&block_hash).unwrap().contains(&peer_ids[1]));
        assert!(non_coords[0].pre_commitments.get(&block_hash).unwrap().contains(&peer_ids[0]));
        assert!(non_coords[0].get_state(block_hash).is_awaiting_commitments());
    }

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
            block_to_propose
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
        assert_eq!(non_coords[0].pre_commitments.get(&block_hash).unwrap().len(), 2);
        assert!(non_coords[0].get_state(block_hash).is_awaiting_precommitments());

        // Getting anther block proposal from the same peer should not change the state
        non_coords[0]
            .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
            .await
            .expect("valid block proposal");
        assert_eq!(non_coords[0].pre_commitments.get(&block_hash).unwrap().len(), 2);
        assert!(non_coords[0].get_state(block_hash).is_awaiting_precommitments());

        // Test that the other non-coord responds the same way
        non_coords[1]
            .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
            .await
            .expect("valid block proposal");
        // There should be two commitments
        assert_eq!(non_coords[1].pre_commitments.get(&block_hash).unwrap().len(), 2);
        assert!(non_coords[1].get_state(block_hash).is_awaiting_precommitments());
    }

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
            block_to_propose
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
        assert_eq!(non_coords[0].pre_commitments.get(&block_hash).unwrap().len(), 2);
        assert!(non_coords[0].get_state(block_hash).is_awaiting_precommitments());

        // Getting anther block proposal from the same peer should not change the state
        non_coords[0]
            .process_block_proposal(block_to_propose.clone(), coord.peer_id.clone())
            .await
            .expect("valid block proposal");
        assert_eq!(non_coords[0].pre_commitments.get(&block_hash).unwrap().len(), 2);
        assert!(non_coords[0].get_state(block_hash).is_awaiting_precommitments());

        let other_peer_id = non_coords[1].peer_id.clone();
        // Process other peers pre-commitment
        non_coords[0]
            .process_precommitment(block_to_propose.clone(), other_peer_id)
            .await
            .expect("valid precommitment");

        // There should be three pre-commitments, non_coord[0], coord which was added at the block proposal stage
        // And non_coord[1] which we just added
        let pre_commitments = non_coords[0].pre_commitments.get(&block_hash).unwrap();
        assert_eq!(pre_commitments.len(), 3);
        for i in 0..pre_commitments.len() {
            assert!(pre_commitments.contains(&peer_ids[i]));
        }
        assert!(non_coords[0].get_state(block_hash).is_awaiting_commitments());

        // Adding the same pre-commit from the same peer shouldnt change anything b/c we are await for commitments
        non_coords[0]
            .process_precommitment(block_to_propose.clone(), other_peer_id)
            .await
            .expect("valid precommitment");
        let mut pre_commitments = non_coords[0].pre_commitments.get(&block_hash).unwrap().clone();
        assert_eq!(pre_commitments.len(), 3);
        for i in 0..pre_commitments.len() {
            assert!(pre_commitments.contains(&peer_ids[i]));
        }
        assert!(non_coords[0].get_state(block_hash).is_awaiting_commitments());

        // Remove the coord's pre-commits
        pre_commitments.remove(&coord.peer_id);
        non_coords[0].pre_commitments.insert(block_hash, pre_commitments.clone());
        non_coords[0].set_state(PbftState::AwaitingPreCommitments, block_hash);
        // Adding the same pre-commit from the same peer shouldnt change anything b/c we are await for commitments
        non_coords[0]
            .process_precommitment(block_to_propose.clone(), other_peer_id)
            .await
            .expect("valid precommitment");

        let pre_commitments = non_coords[0].pre_commitments.get(&block_hash).unwrap().clone();
        assert_eq!(pre_commitments.len(), 2);
        // At this point its us (non_coord[0]) and the other peer (non_coord[1)
        assert!(pre_commitments.contains(&non_coords[0].peer_id));
        assert!(pre_commitments.contains(&non_coords[1].peer_id));
        assert!(non_coords[0].get_state(block_hash).is_awaiting_precommitments());
    }

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
            block_to_propose
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

        coord
            .process_commitment(signed_block_0.clone(), peer_id_0)
            .await
            .expect("valid commitment");
        // Coordinator should still be awaiting commitments
        assert!(coord.get_state(block_hash).is_awaiting_commitments());
        let sigs_so_far = coord
            .sealed_blocks
            .get(&block_hash)
            .unwrap()
            .header()
            .clone()
            .deserialize_extra_data_header()
            .unwrap()
            .authority_signatures
            .expect("should have signatures");

        assert_eq!(sigs_so_far.len(), 2);

        // adding the same commitment should not change anything
        coord
            .process_commitment(signed_block_0.clone(), peer_id_0)
            .await
            .expect("valid commitment");
        let sigs_again = coord
            .sealed_blocks
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
                let msg = secp256k1::Message::from_slice(
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
}
