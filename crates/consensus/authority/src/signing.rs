use crate::{
    frost_task::FrostNotification,
    random_source_provider::RandomSource,
    utils::{
        deserialize_frost_peer_id, parse_signing_session_id, retry_exec, retry_future,
        FrostParseError,
    },
};
use btcserverlib::extended_client::{BtcServerExtendedClient, GrpcClientError};
use client::{Empty, FinalizeSigningResponse, SigningPackage, SigningPackageRequest};
use frost_secp256k1_tr as frost;

use reth_chainspec::ChainSpec;
use reth_consensus_common::utils::{current_inturn_index, is_inturn, unix_timestamp};
use reth_network::frost::{
    manager::{
        authority_index_to_frost_identifier, FrostCommand, FrostConfig, PeerData, ToFrostManager,
    },
    FrostPeerCommand, PeerMessageResponse, SigningEventResponseType, SigningResponse,
};
use reth_revm::primitives::FixedBytes;
use reth_rpc_types::PeerId;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{mpsc::error::SendError, RwLock};
use tracing::{error, info, warn};

type SigningStatesMap = Arc<RwLock<HashMap<[u8; 32], SigningSession>>>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Internal gRPC error: message: {0}, status: {1}")]
    InternalGrpc(String, tonic::Status),
    #[error("Invalid frost peer id")]
    InvalidFrostPeerId,
    #[error("Invalid signing session id")]
    InvalidSigningSessionId,
    #[error("Failed to get peers handles")]
    FailedToGetPeersHandles,
    #[error("Not enough connected peers to gossip to")]
    NotEnoughConnectedPeers,
    #[error("Coordinator re-triggered an existing signing session")]
    CoordinatorRetriggeredSession,
    #[error("Send error: {0}")]
    Send(#[from] SendError<FrostPeerCommand>),
}

impl From<FrostParseError> for Error {
    fn from(value: FrostParseError) -> Self {
        match value {
            FrostParseError::InvalidFrostPeerId => Error::InvalidFrostPeerId,
            FrostParseError::InvalidSigningSessionId => Error::InvalidSigningSessionId,
        }
    }
}

impl From<GrpcClientError> for Error {
    fn from(value: GrpcClientError) -> Self {
        Error::InternalGrpc(value.to_string(), value.to_tonic_status())
    }
}

/// Defines the states of the state machine for a session id
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SigningState {
    /// The initial signing state (unstarted)
    Initial,
    /// Round 1 of signing has been started
    Round1,
    /// Round 2 of signing has been started
    Round2,
    /// Finalized
    Finalized,
    /// The signing state machine has failed
    Failed,
}

impl SigningState {
    #[allow(dead_code)]
    /// Returns true if the signing state machine is in a running state
    pub(crate) fn is_running(&self) -> bool {
        !matches!(self, SigningState::Initial | SigningState::Finalized | SigningState::Failed)
    }
    /// Returns true if we are in round 1 of the signing
    pub(crate) fn is_round1(&self) -> bool {
        matches!(self, SigningState::Round1)
    }
    /// Returns true if we are in round 2 of signing
    pub(crate) fn is_round2(&self) -> bool {
        matches!(self, SigningState::Round2)
    }

    #[allow(dead_code)]
    /// Returns true if we are in a finalized signing state
    pub(crate) fn is_finalized(&self) -> bool {
        matches!(self, SigningState::Finalized)
    }

    #[warn(dead_code)]
    /// Returns true if the signing has failed
    pub(crate) fn has_failed(&self) -> bool {
        matches!(self, SigningState::Failed)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SigningSession {
    #[allow(dead_code)]
    /// The id of the signing session
    session_id: [u8; 32],
    /// The state of the session
    state: SigningState,
    /// The index of the session coordinator
    coordinator_index: u64,
    /// The original session payload
    original_psbt: Option<Vec<u8>>,
}

/// A state machine for transitioning between different signing states
#[derive(Debug)]
pub(crate) struct SigningStateMachine<ToFrostMan, Source> {
    chain_spec: Arc<ChainSpec>,
    btc_client: BtcServerExtendedClient,
    frost_handle: ToFrostMan,
    signing_states: Arc<RwLock<HashMap<[u8; 32], SigningSession>>>,
    personal_frost_identifier: frost::Identifier,
    frost_config: FrostConfig,
    random_source_provider: Source,
}

impl<ToFrostMan, Source> SigningStateMachine<ToFrostMan, Source>
where
    ToFrostMan: ToFrostManager + Clone,
    Source: RandomSource,
{
    /// Constructs a new state machine with the given params
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        btc_client: BtcServerExtendedClient,
        frost_handle: ToFrostMan,
        frost_config: FrostConfig,
        random_source_provider: Source,
    ) -> Self {
        let personal_frost_identifier: frost::Identifier =
            authority_index_to_frost_identifier(frost_config.authority_index as u16);

        let signing_states: SigningStatesMap = Arc::new(RwLock::new(HashMap::default()));

        Self {
            chain_spec,
            btc_client,
            frost_handle,
            signing_states,
            personal_frost_identifier,
            frost_config,
            random_source_provider,
        }
    }

    /// Inserts a signing state into the state machine
    pub(crate) async fn update_signing_state(
        &mut self,
        session_id: [u8; 32],
        signing_state: SigningState,
    ) {
        if !self.signing_states.read().await.contains_key(&session_id) {
            return;
        }
        if let Some(signing_session) = self.signing_states.write().await.get_mut(&session_id) {
            signing_session.state = signing_state;
        }
    }

    /// Checks if a signing session exists or not yet
    pub(crate) async fn signing_session_exists(&mut self, session_id: [u8; 32]) -> bool {
        self.signing_states.read().await.contains_key(&session_id)
    }

    /// Inserts a new signing session
    pub(crate) async fn insert_new_signing_session(
        &mut self,
        session_id: [u8; 32],
        coordinator_index: u64,
        original_psbt: Option<Vec<u8>>,
        signing_state: SigningState,
    ) {
        if self.signing_states.read().await.contains_key(&session_id) {
            return;
        }
        self.signing_states.write().await.insert(
            session_id,
            SigningSession { session_id, state: signing_state, coordinator_index, original_psbt },
        );
    }

    /// Returns the original psbt into the state machine
    pub(crate) async fn get_signing_session(&self, session_id: [u8; 32]) -> Option<SigningSession> {
        self.signing_states.read().await.get(&session_id).cloned()
    }

    /// Removes a signing session
    pub(crate) async fn remove_signing_session(
        &mut self,
        session_id: [u8; 32],
    ) -> Option<SigningSession> {
        self.signing_states.write().await.remove(&session_id)
    }

    /// Check if the session id is in a failed state
    pub(crate) async fn is_failed_state(&self, session_id: &[u8; 32]) -> bool {
        self.signing_states
            .read()
            .await
            .get(session_id)
            .map(|signing_session| signing_session.state.has_failed())
            .unwrap_or_default()
    }

    /// Check if the session id is in a round1 state
    pub(crate) async fn is_round1_state(&self, session_id: &[u8; 32]) -> bool {
        self.signing_states
            .read()
            .await
            .get(session_id)
            .map(|signing_session| signing_session.state.is_round1())
            .unwrap_or_default()
    }

    /// Check if the session id is in a round2 state
    pub(crate) async fn is_round2_state(&self, session_id: &[u8; 32]) -> bool {
        self.signing_states
            .read()
            .await
            .get(session_id)
            .map(|signing_session| signing_session.state.is_round2())
            .unwrap_or_default()
    }
}

impl<ToFrostMan, Source> SigningStateMachine<ToFrostMan, Source>
where
    ToFrostMan: ToFrostManager + Clone,
    Source: RandomSource,
{
    async fn get_round1_signing_package(
        &mut self,
        signing_session_id: FixedBytes<32>,
        psbt: Vec<u8>,
    ) -> Result<SigningPackage, Error> {
        let round1_payload = self
            .btc_client
            .get_round1_signing_package(SigningPackageRequest {
                psbt,
                signing_session_id: signing_session_id.to_vec(),
            })
            .await;

        let round1_payload = match round1_payload {
            Ok(round1_payload) => round1_payload,
            Err(e) => return Err(Error::from(e)),
        };
        Ok(round1_payload)
    }

    async fn get_round2_signing_package(
        &mut self,
        signing_session_id: FixedBytes<32>,
        psbt: Vec<u8>,
    ) -> Result<SigningPackage, Error> {
        let round2_payload = self
            .btc_client
            .get_round2_signing_package(SigningPackageRequest {
                psbt,
                signing_session_id: signing_session_id.to_vec(),
            })
            .await;

        let round2_payload = match round2_payload {
            Ok(round2_payload) => round2_payload,
            Err(e) => return Err(Error::from(e)),
        };
        Ok(round2_payload)
    }

    async fn new_round1_signing_package(
        &mut self,
        identifier: Vec<u8>,
        signing_session_id: FixedBytes<32>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let new_round1_signing_package = self
            .btc_client
            .new_round1_signing_package(SigningPackage {
                identifier,
                psbt,
                signing_session_id: signing_session_id.to_vec(),
            })
            .await;

        match new_round1_signing_package {
            Ok(_) => {}
            Err(e) => return Err(Error::from(e)),
        };
        Ok(())
    }

    async fn new_round2_signing_package(
        &mut self,
        identifier: Vec<u8>,
        signing_session_id: FixedBytes<32>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let new_round1_signing_package = self
            .btc_client
            .new_round2_signing_package(SigningPackage {
                identifier,
                psbt,
                signing_session_id: signing_session_id.to_vec(),
            })
            .await;

        match new_round1_signing_package {
            Ok(_) => {}
            Err(e) => return Err(Error::from(e)),
        };
        Ok(())
    }

    async fn get_to_sign_package(
        &mut self,
        signing_session_id: FixedBytes<32>,
    ) -> Result<SigningPackage, Error> {
        let package = match self
            .btc_client
            .get_to_sign_package(client::ToSignRequest {
                signing_session_id: signing_session_id.to_vec(),
            })
            .await
        {
            Ok(sign_payload) => sign_payload,
            Err(e) => return Err(Error::from(e)),
        };
        Ok(package)
    }

    async fn finalize_signing(
        &mut self,
        signing_session_id: FixedBytes<32>,
    ) -> Result<FinalizeSigningResponse, Error> {
        let finalized_signing = match self
            .btc_client
            .finalize_signing(client::FinalizeSigningRequest {
                signing_session_id: signing_session_id.to_vec(),
            })
            .await
        {
            Ok(finalized_signing) => finalized_signing,
            Err(e) => return Err(Error::from(e)),
        };
        Ok(finalized_signing)
    }

    async fn abort_signing(&mut self) -> Result<(), Error> {
        match self.btc_client.abort_signing(Empty {}).await {
            Ok(_) => {}
            Err(e) => return Err(Error::from(e)),
        };
        Ok(())
    }

    pub(crate) async fn get_all_peers_handle(&self) -> Result<HashMap<PeerId, PeerData>, Error> {
        // get all frost peers connections
        let (peers_connections_sender, peers_connections_receiver) =
            tokio::sync::oneshot::channel::<HashMap<PeerId, PeerData>>();
        if let Err(e) = self
            .frost_handle
            .send_command(FrostCommand::GetAllConnectedPeers(peers_connections_sender))
        {
            error!(target: "consensus::authority::signing", "Failed to send GetAllConnectedPeers frost message {:?}", e);
        }
        match peers_connections_receiver.await {
            Ok(connected_peers) => Ok(connected_peers),
            Err(e) => {
                error!(target: "consensus::authority::signing", "Failed to get frost peers connections {:?}", e);
                Err(Error::FailedToGetPeersHandles)
            }
        }
    }

    /// Gets the current federation coordinator. Returns None if it is us, otherwise Some if someone
    /// Uses a random 32 byte source to determine the current inturn authority
    pub(crate) async fn get_coordinator_peer_data(&self) -> Result<Option<(PeerData, u64)>, Error> {
        // check if we are in turn
        let leader_selection_window = self
            .chain_spec
            .leader_selection_window
            .expect("block times to be set for PoA consensus");

        let is_inturn = is_inturn(
            self.frost_config.authorities.len() as u64,
            self.frost_config.authority_index as u64,
            leader_selection_window,
            self.random_source_provider.random_source(),
        );
        match is_inturn {
            true => {
                // if we are inturn, return None to avoid sending messages to ourselves.
                Ok(None)
            }
            false => {
                // if we are not inturn, find the coordinator in the list of peers
                let all_connected_frost_peers = self.get_all_peers_handle().await?;
                let current_inturn_authority_index = current_inturn_index(
                    self.frost_config.authorities.len() as u64,
                    unix_timestamp(),
                    leader_selection_window,
                );
                let current_inturn_authority_frost_identifier =
                    authority_index_to_frost_identifier(current_inturn_authority_index as u16);
                let coord = all_connected_frost_peers.iter().find_map(|(_peer_id, peer_data)| {
                    if peer_data.frost_identifier == current_inturn_authority_frost_identifier {
                        Some(peer_data.clone())
                    } else {
                        None
                    }
                });

                Ok(coord.zip(Some(current_inturn_authority_index)))
            }
        }
    }

    /// Returns if we are a coordinator or not
    pub(crate) fn is_coordinator(&self) -> bool {
        let leader_selection_window = self
            .chain_spec
            .leader_selection_window
            .expect("block times to be set for PoA consensus");
        is_inturn(
            self.frost_config.authorities.len() as u64,
            self.frost_config.authority_index as u64,
            leader_selection_window,
            self.random_source_provider.random_source(),
        )
    }

    pub(crate) async fn gossip_to_peers(
        &mut self,
        signing_package: SigningPackage,
        my_frost_identifier: Vec<u8>,
        response_type: SigningEventResponseType,
    ) -> Result<(), Error> {
        let SigningPackage { identifier: _, signing_session_id, psbt } = signing_package;

        let fut = || async {
            // get all connected peers
            let connected_peers = self.get_all_peers_handle().await?;

            // check if we have enough connected peers to gossip to and include ourselves
            if connected_peers.len() + 1 < self.frost_config.min_signers as usize {
                error!(target: "consensus::authority::signing", "Not enough connected peers to gossip to");
                return Err(Error::NotEnoughConnectedPeers);
            }

            // Broadcast signing round 2 package to all peers (excluding ourselves)
            for (_peer_id, connected_peer) in connected_peers.iter() {
                if connected_peer.frost_identifier != self.personal_frost_identifier {
                    let resp = PeerMessageResponse::Signing(SigningResponse {
                        response_type,
                        identifier: my_frost_identifier.clone(),
                        signing_session_id: signing_session_id.clone(),
                        psbt: psbt.clone(),
                    });
                    connected_peer
                        .peer_commands_tx
                        .send(FrostPeerCommand::PeerMessage(resp))
                        .map_err(|e| {
                            error!(target: "consensus::authority::signing", "Failed to send PeerMessage {:?}", e.to_string());
                            Error::Send(e)
                    })?;
                }
            }
            Ok(())
        };

        retry_exec("gossip_to_peers", fut, 3, Duration::from_millis(500)).await
    }

    // ====================================== 1 =========================================
    // Coordinator initiates a new signing session
    pub(crate) async fn initate_signing_session(
        &mut self,
        signing_session_id: FixedBytes<32>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let session_id = parse_signing_session_id(&signing_session_id)?;

        // the coordinator is always expected to be us in this case, i.e. None
        let coordinator = self.get_coordinator_peer_data().await?;
        if coordinator.is_some() {
            error!(target: "consensus::authority::signing::initate_signing_session", "A non-coordinator is trying to (re)initiate a signing process!");
            return Ok(());
        }

        // checking coordinator is not re-triggering an existing session
        // if not, start new session
        match self.get_signing_session(session_id).await {
            Some(signing_session) => {
                // a coordinator should never re-trigger an existing session
                if signing_session.coordinator_index == self.frost_config.authority_index as u64 {
                    // clear session and lose ability to be coordinator
                    // this could happen if previous session failed but wasn't removed
                    self.abort_signing().await?;
                    self.remove_signing_session(session_id).await;

                    error!(target: "consensus::authority::signing::initate_signing_session", "A coordinator re-triggered an existing signing session!");
                    return Err(Error::CoordinatorRetriggeredSession);
                }
            }
            None => {
                // clear any existing session and start a new one
                self.abort_signing().await?;
                self.insert_new_signing_session(
                    session_id,
                    self.frost_config.authority_index as u64,
                    Some(psbt.clone()),
                    SigningState::Initial,
                )
                .await;
            }
        }

        info!(target: "consensus::authority::signing::initate_signing_session", "starting signing session with id {:?}", session_id);

        // As the cord we generate round 1 nonces and save them
        // then we send the psbt to other peers
        let signing_round1_package =
            self.get_round1_signing_package(signing_session_id, psbt).await?;
        self.new_round1_signing_package(
            self.personal_frost_identifier.serialize().to_vec(),
            signing_session_id,
            signing_round1_package.clone().psbt,
        )
        .await?;

        // send to all other peers
        self.update_signing_state(session_id, SigningState::Round1).await;
        if let Err(e) = self
            .gossip_to_peers(
                signing_round1_package,
                self.personal_frost_identifier.serialize().to_vec(),
                SigningEventResponseType::SignerRound1SigningPackage,
            )
            .await
        {
            error!(target: "consensus::authority::signing::initate_signing_session", "Error gossiping round 1 to peers {:?}", e);
            self.update_signing_state(session_id, SigningState::Failed).await;
            return Err(e);
        }
        Ok(())
    }

    /// A signer processes round 1 signing packages
    pub(crate) async fn signer_process_round1(
        &mut self,
        identifier: Vec<u8>,
        signing_session_id: FixedBytes<32>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let session_id = parse_signing_session_id(&signing_session_id)?;

        // get coordinator, and check if we are the coordinator
        let (coordinator_peer_data, coordinator_id) = match self.get_coordinator_peer_data().await?
        {
            Some(coord_data) => (coord_data.0, coord_data.1),
            None => {
                info!(target: "consensus::authority::signing::signer_process_round1", "we are the coordinator");
                return Ok(());
            }
        };

        // check coordinator is sending the request
        let coordinator_frost_identifier = coordinator_peer_data.frost_identifier;
        if coordinator_frost_identifier.serialize().to_vec() != identifier {
            warn!(target: "consensus::authority::signing::signer_process_round1", "Round 1 signing request not from coordinator");
            return Ok(());
        }

        // no already existing signing session found
        if !self.signing_session_exists(session_id).await {
            // insert a new signing session
            self.insert_new_signing_session(session_id, coordinator_id, None, SigningState::Round1)
                .await;
            // abort any previous session
            // coordinator should only send this request once and should always be in round 1
            self.abort_signing().await?;
        } else {
            // NOTE: if a session exists, the current coordinator should not be sending a request to
            // start a new session. The previous session should have been successful or
            // failed both leading to the session being removed
            error!(target: "consensus::authority::signing::signer_process_round1", "Coordinator re-triggered an existing signing session!");
            self.remove_signing_session(session_id).await;
            return Err(Error::CoordinatorRetriggeredSession);
        }

        // add the transmitted round 1 package data (the original psbt package - there is only 1x of
        // them)
        let signing_package_round1 = match self
            .get_round1_signing_package(signing_session_id, psbt)
            .await
        {
            Ok(signing_package_round1) => signing_package_round1,
            Err(e) => {
                error!(target: "consensus::authority::signing::signer_process_round1", "Error adding round 2 signing package {:?}", e);
                self.update_signing_state(session_id, SigningState::Failed).await;
                return Err(e);
            }
        };
        // Update signing state
        self.update_signing_state(session_id, SigningState::Round2).await;

        // Broadcast signing round 1 to the coordinator
        if coordinator_frost_identifier != self.personal_frost_identifier {
            let resp = PeerMessageResponse::Signing(SigningResponse {
                response_type: SigningEventResponseType::CoordinatorRound1SigningPackage,
                identifier: signing_package_round1.identifier.clone(),
                signing_session_id: signing_package_round1.signing_session_id.clone(),
                psbt: signing_package_round1.psbt.clone(),
            });

            retry_future(
                || {
                    let sender = coordinator_peer_data.peer_commands_tx.clone();
                    let message = resp.clone();
                    async move {
                        sender.send(FrostPeerCommand::PeerMessage(message)).map_err(Error::Send)
                    }
                },
                3,
                Duration::from_secs(1),
            )
            .await?
        }

        Ok(())
    }

    /// A coordinator processes round 1 signing packages
    pub(crate) async fn coordinator_process_round1(
        &mut self,
        identifier: Vec<u8>,
        signing_session_id: FixedBytes<32>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let session_id = parse_signing_session_id(&signing_session_id)?;

        info!(target: "consensus::authority::signing::coordinator_process_round1", "signing session id {:?}", session_id);
        // return if we are not in round 1 or not a coordinator
        if !self.is_round1_state(&session_id).await {
            warn!(target: "consensus::authority::signing::coordinator_process_round1", "is not in round1");
            return Ok(());
        }
        if !self.is_coordinator() {
            warn!(target: "consensus::authority::signing::coordinator_process_round1", "we are not the coordinator");
            return Ok(());
        }

        info!(
            target: "consensus::authority::signing::coordinator_process_round1",
            "identifiers my peer id: {:?}, other peerid {:?}",
            self.personal_frost_identifier,
            deserialize_frost_peer_id(identifier.clone())?
        );

        // return if the sending identifier is us
        if self.personal_frost_identifier == deserialize_frost_peer_id(identifier.clone())? {
            return Ok(());
        }

        // add the transmitted round 1 package data
        if let Err(e) =
            self.new_round1_signing_package(identifier.clone(), signing_session_id, psbt).await
        {
            error!(target: "consensus::authority::signing::coordinator_process_round1","Error adding round 1 signing package {:?}", e);
            return Ok(());
        }

        // try to generate signing package
        if let Ok(to_sign_payload) = self.get_to_sign_package(signing_session_id).await {
            // we should add the cord partial sig
            let cord_round2 = self
                .get_round2_signing_package(signing_session_id, to_sign_payload.psbt.clone())
                .await?;
            self.new_round2_signing_package(
                self.personal_frost_identifier.serialize().to_vec(),
                signing_session_id,
                cord_round2.psbt,
            )
            .await?;

            self.update_signing_state(session_id, SigningState::Round2).await;
            // if ok, send to all peers
            // TODO we really just need to send to all signers that responded to the round 1
            if let Err(e) = self
                .gossip_to_peers(
                    to_sign_payload.clone(),
                    self.personal_frost_identifier.serialize().to_vec(),
                    SigningEventResponseType::SignerRound2SigningPackage,
                )
                .await
            {
                error!(target: "consensus::authority::signing::coordinator_process_round1", "Error gossiping round 2 to peers {:?}", e);
                self.update_signing_state(session_id, SigningState::Failed).await;
                return Err(e);
            }
            info!(target: "consensus::authority::signing::coordinator_process_round1", "to sign payload send to signers");
        }

        Ok(())
    }

    // ====================================== 2 =========================================

    /// A signer processes round 2 signing request
    pub(crate) async fn signer_process_round2(
        &mut self,
        identifier: Vec<u8>,
        signing_session_id: FixedBytes<32>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let session_id = parse_signing_session_id(&signing_session_id)?;

        // get coordinator
        let (coordinator_peer_data, coordinator_id) = match self.get_coordinator_peer_data().await?
        {
            Some(coord_data) => (coord_data.0, coord_data.1),
            None => {
                // return if we are a coordinator
                warn!(
                    target: "consensus::authority::signing::signer_process_round2",
                    "we are the coordinator",
                );
                return Ok(());
            }
        };
        info!(target: "consensus::authority::signing::signer_process_round2", "coordinator index {:?}", coordinator_id);

        // check coordinator is sending the request
        let coordinator_frost_identifier = coordinator_peer_data.frost_identifier;
        if coordinator_frost_identifier.serialize().to_vec() != identifier {
            warn!(target: "consensus::authority::signing::signer_process_round2", "Round 2 signing request not from coordinator");
            return Ok(());
        }

        // return if we are not in round 2
        if !self.is_round2_state(&session_id).await {
            warn!(target: "consensus::authority::signing::signer_process_round2", "is not in round2");
            return Ok(());
        }

        // add the transmitted round 2 package data
        let signing_package_round2 =
            match self.get_round2_signing_package(signing_session_id, psbt).await {
                Ok(signing_package_round2) => signing_package_round2,
                Err(e) => {
                    error!("Error adding round 2 signing package {:?}", e);
                    self.update_signing_state(session_id, SigningState::Failed).await;
                    return Err(e);
                }
            };

        // Broadcast signing round 2 to the coordinator

        let resp = PeerMessageResponse::Signing(SigningResponse {
            response_type: SigningEventResponseType::CoordinatorRound2SigningPackage,
            identifier: signing_package_round2.identifier.clone(),
            signing_session_id: signing_package_round2.signing_session_id.clone(),
            psbt: signing_package_round2.psbt.clone(),
        });

        retry_future(
                || {
                    let sender = coordinator_peer_data.peer_commands_tx.clone();
                    let message = resp.clone();
                    async move {
                        sender.send(FrostPeerCommand::PeerMessage(message)).map_err(Error::Send)
                    }
                },
                3,
                Duration::from_secs(1),
            )
            .await?;

        Ok(())
    }

    /// A coordinator processes round 2 signing packages
    pub(crate) async fn coordinator_process_round2(
        &mut self,
        identifier: Vec<u8>,
        signing_session_id: FixedBytes<32>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let session_id = parse_signing_session_id(&signing_session_id)?;

        // return if we are not in round 2 or not a coordinator
        if !self.is_round2_state(&session_id).await {
            warn!(target: "consensus::authority::signing::coordinator_process_round2", "is not in round2");
            return Ok(());
        }

        // return if we are not a coordinator
        if !self.is_coordinator() {
            warn!(
                target: "consensus::authority::signing::coordinator_process_round2",
                "we are not the coordinator",
            );
            return Ok(());
        }

        info!(
            target: "consensus::authority::signing::coordinator_process_round2",
            "My identifier {:?} and the peers identifier {:?}",
            self.personal_frost_identifier,
            deserialize_frost_peer_id(identifier.clone())?
        );

        // return if the sending identifier is us
        if self.personal_frost_identifier == deserialize_frost_peer_id(identifier.clone())? {
            info!(target: "consensus::authority::signing::coordinator_process_round2", "identifier is us, this should not happen");
            return Ok(());
        }

        // add the transmitted round 2 package data
        if let Err(e) =
            self.new_round2_signing_package(identifier.clone(), signing_session_id, psbt).await
        {
            error!(target: "consensus::authority::signing::coordinator_process_round2", "Error adding round 2 signing package {:?}", e);
            self.update_signing_state(session_id, SigningState::Failed).await;
            return Err(e);
        }
        info!(target: "consensus::authority::signing::coordinator_process_round2", "round 2 added");

        // try to finalize the signing
        if let Ok(_sign_payload) = self.finalize_signing(signing_session_id).await {
            info!(target: "consensus::authority::signing::coordinator_process_round2", "signing finalized!");
            self.update_signing_state(session_id, SigningState::Finalized).await
        }

        Ok(())
    }
}
