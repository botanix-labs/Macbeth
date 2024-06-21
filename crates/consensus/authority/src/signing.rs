use crate::{
    extended_client::BtcServerExtendedClient,
    frost_task::{FrostNotification, FrostNotificationMessage},
    utils::{
        deserialize_frost_peer_id, parse_signing_session_id, retry_exec, retry_future,
        FrostParseError,
    },
    Storage, BLOCK_TIME_DURATION_SECS,
};
use client::{Empty, FinalizeSigningResponse, SigningPackage, SigningPackageRequest};
use frost_secp256k1_tr as frost;
use reth_consensus_common::utils::{
    current_inturn_index, get_in_turn_interval, is_inturn, unix_timestamp, CoordinatorInterval,
};
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_network::frost::{
    manager::{peer_id_to_identifier, FrostCommand, FrostConfig, PeerData, ToFrostManager},
    FrostPeerCommand, PeerMessageResponse, SigningEventResponseType, SigningResponse,
};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use reth_rpc_types::PeerId;
use reth_tasks::TaskExecutor;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{
    mpsc::{error::SendError, UnboundedSender},
    RwLock,
};
use tracing::{error, info, warn};

type SigningStatesMap = Arc<RwLock<HashMap<[u8; 32], SigningSession>>>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Unknown internal error")]
    InternalGrpc,
    #[error("Failed to get connected peers handles")]
    FailedToGetConnectedPeersHandles,
    #[error("Invalid frost peer id")]
    InvalidFrostPeerId,
    #[error("Failed to parse signing session id")]
    FailedToParseSigningSessionId,
    #[error("Failed to deserialize psbt")]
    FailedToDeserializePsbt,
    #[error("Failed to get round 1 signing package")]
    FailedToGetRound1SigningPackage,
    #[error("Failed to get round 2 signing package")]
    FailedToGetRound2SigningPackage,
    #[error("Failed to add round 1 signing package")]
    FailedToAddRound1SigningPackage,
    #[error("Failed to add round 2 signing package")]
    FailedToAddRound2SigningPackage,
    #[error("Failed to serialize psbt")]
    FailedToSerializePsbt,
    #[error("Failed to finalize signing")]
    FailedToFinalizeSigning,
    #[error("Failed to parse frost peer id")]
    FailedToParseFrostPeerId,
    #[error("Failed to get to sign")]
    FailedToGetToSign,
    #[error("Invalid signing session id")]
    InvalidSigningSessionId,
    #[error("Failed to send peer command {0}")]
    Send(SendError<FrostPeerCommand>),
    #[error("Missing key package")]
    MissingKeyPackage,
}

impl From<FrostParseError> for Error {
    fn from(value: FrostParseError) -> Self {
        match value {
            FrostParseError::InvalidFrostPeerId => Error::InvalidFrostPeerId,
            FrostParseError::InvalidSigningSessionId => Error::InvalidSigningSessionId,
        }
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
    #[warn(dead_code)]
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

    #[warn(dead_code)]
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
    /// The id of the signing session
    session_id: [u8; 32],
    /// The state of the session
    state: SigningState,
    /// The index of the session coordinator
    coordinator_index: u64,
    /// The original session payload
    original_psbt: Option<Vec<u8>>,
    /// Validity from
    validity_from: u64,
    /// Validity to
    validity_to: u64,
}

/// A state machine for transitioning between different signing states
#[derive(Debug)]
pub(crate) struct SigningStateMachine<Client, ToFrostMan> {
    btc_client: BtcServerExtendedClient,
    storage: Storage<Client>,
    frost_handle: ToFrostMan,
    signing_states: Arc<RwLock<HashMap<[u8; 32], SigningSession>>>,
    personal_frost_identifier: frost::Identifier,
    frost_config: FrostConfig,
    frost_task_tx: UnboundedSender<FrostNotificationMessage>,
}

impl<Client, ToFrostMan> SigningStateMachine<Client, ToFrostMan>
where
    ToFrostMan: ToFrostManager + Clone,
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
{
    /// Constructs a new state machine with the given params
    pub(crate) fn new(
        btc_client: BtcServerExtendedClient,
        storage: Storage<Client>,
        frost_handle: ToFrostMan,
        frost_config: FrostConfig,
        frost_task_tx: UnboundedSender<FrostNotificationMessage>,
        task_executor: TaskExecutor,
    ) -> Self {
        let personal_frost_identifier: frost::Identifier =
            peer_id_to_identifier(frost_config.authority_index as u16);

        let signing_states: SigningStatesMap = Arc::new(RwLock::new(HashMap::default()));
        let signing_states_clone = Arc::clone(&signing_states);
        let sleep_duration = Duration::from_secs(2 * BLOCK_TIME_DURATION_SECS);
        task_executor.spawn(async move {
            loop {
                // remove stale signing sessions
                let mut guard = signing_states_clone.write().await;
                guard.retain(|_, signing_session| {
                    signing_session.validity_to >= unix_timestamp() - sleep_duration.as_secs()
                });
                drop(guard);

                // sleep until next cleanup round
                tokio::time::sleep(sleep_duration).await;
            }
        });

        Self {
            btc_client,
            storage,
            frost_handle,
            signing_states,
            personal_frost_identifier,
            frost_config,
            frost_task_tx,
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
        validity_from: u64,
        validity_to: u64,
    ) {
        if self.signing_states.read().await.contains_key(&session_id) {
            return;
        }
        self.signing_states.write().await.insert(
            session_id,
            SigningSession {
                session_id,
                state: signing_state,
                coordinator_index,
                original_psbt,
                validity_from,
                validity_to,
            },
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

impl<Client, ToFrostMan> SigningStateMachine<Client, ToFrostMan>
where
    ToFrostMan: ToFrostManager + Clone,
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
{
    async fn get_round1_signing_package(
        &mut self,
        signing_session_id: Vec<u8>,
        psbt: Vec<u8>,
    ) -> Result<SigningPackage, Error> {
        let round1_payload = self
            .btc_client
            .get_round1_signing_package(SigningPackageRequest { psbt, signing_session_id })
            .await;

        let round1_payload = match round1_payload {
            Ok(round1_payload) => round1_payload,
            Err(e) => {
                let status = e.to_tonic_status();
                match status.code() {
                    tonic::Code::InvalidArgument
                        if status.message().contains("Failed to parse signing session id") =>
                    {
                        return Err(Error::FailedToParseSigningSessionId)
                    }
                    tonic::Code::Internal
                        if status.message().contains("Failed to deserialize psbt") =>
                    {
                        return Err(Error::FailedToDeserializePsbt)
                    }
                    tonic::Code::Internal
                        if status.message().contains("Failed to get round1 signing package") =>
                    {
                        return Err(Error::FailedToGetRound1SigningPackage)
                    }
                    tonic::Code::Internal
                        if status.message().contains("Failed to serialize psbt") =>
                    {
                        return Err(Error::FailedToSerializePsbt)
                    }
                    _ => return Err(Error::InternalGrpc),
                }
            }
        };
        Ok(round1_payload)
    }

    async fn get_round2_signing_package(
        &mut self,
        signing_session_id: Vec<u8>,
        psbt: Vec<u8>,
    ) -> Result<SigningPackage, Error> {
        let round2_payload = self
            .btc_client
            .get_round2_signing_package(SigningPackageRequest { psbt, signing_session_id })
            .await;

        let round2_payload = match round2_payload {
            Ok(round2_payload) => round2_payload,
            Err(e) => {
                let e = e.to_tonic_status();
                match e.code() {
                    tonic::Code::InvalidArgument
                        if e.message().contains("Failed to parse signing session id") =>
                    {
                        return Err(Error::FailedToParseSigningSessionId)
                    }
                    tonic::Code::Internal if e.message().contains("Failed to deserialize psbt") => {
                        return Err(Error::FailedToDeserializePsbt)
                    }
                    tonic::Code::Internal
                        if e.message().contains("Failed to get round2 signing package") =>
                    {
                        return Err(Error::FailedToGetRound2SigningPackage)
                    }
                    tonic::Code::Internal if e.message().contains("Failed to serialize psbt") => {
                        return Err(Error::FailedToSerializePsbt)
                    }
                    _ => return Err(Error::InternalGrpc),
                }
            }
        };
        Ok(round2_payload)
    }

    async fn new_round1_signing_package(
        &mut self,
        identifier: Vec<u8>,
        signing_session_id: Vec<u8>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let new_round1_signing_package = self
            .btc_client
            .new_round1_signing_package(SigningPackage { identifier, psbt, signing_session_id })
            .await;

        match new_round1_signing_package {
            Ok(_) => {}
            Err(e) => {
                let e = e.to_tonic_status();
                match e.code() {
                    tonic::Code::InvalidArgument
                        if e.message().contains("Failed to parse signing session id") =>
                    {
                        return Err(Error::FailedToParseSigningSessionId)
                    }
                    tonic::Code::InvalidArgument
                        if e.message().contains("Failed to parse frost peer id") =>
                    {
                        return Err(Error::FailedToParseFrostPeerId)
                    }
                    tonic::Code::Internal if e.message().contains("Failed to deserialize psbt") => {
                        return Err(Error::FailedToDeserializePsbt)
                    }
                    tonic::Code::InvalidArgument
                        if e.message().contains("Failed to add round1 signing") =>
                    {
                        return Err(Error::FailedToAddRound1SigningPackage)
                    }
                    _ => return Err(Error::InternalGrpc),
                }
            }
        };
        Ok(())
    }

    async fn new_round2_signing_package(
        &mut self,
        identifier: Vec<u8>,
        signing_session_id: Vec<u8>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let new_round1_signing_package = self
            .btc_client
            .new_round2_signing_package(SigningPackage { identifier, psbt, signing_session_id })
            .await;

        match new_round1_signing_package {
            Ok(_) => {}
            Err(e) => {
                let e = e.to_tonic_status();
                match e.code() {
                    tonic::Code::InvalidArgument
                        if e.message().contains("Failed to parse signing session id") =>
                    {
                        return Err(Error::FailedToParseSigningSessionId)
                    }
                    tonic::Code::InvalidArgument
                        if e.message().contains("Failed to parse frost peer id") =>
                    {
                        return Err(Error::FailedToParseFrostPeerId)
                    }
                    tonic::Code::Internal if e.message().contains("Failed to deserialize psbt") => {
                        return Err(Error::FailedToDeserializePsbt)
                    }
                    tonic::Code::InvalidArgument
                        if e.message().contains("Failed to add round1 signing") =>
                    {
                        return Err(Error::FailedToAddRound2SigningPackage)
                    }
                    _ => return Err(Error::InternalGrpc),
                }
            }
        };
        Ok(())
    }

    async fn get_to_sign_package(
        &mut self,
        signing_session_id: Vec<u8>,
    ) -> Result<SigningPackage, Error> {
        let package = match self
            .btc_client
            .get_to_sign_package(client::ToSignRequest { signing_session_id })
            .await
        {
            Ok(sign_payload) => sign_payload,
            Err(e) => {
                let e = e.to_tonic_status();
                match e.code() {
                    tonic::Code::InvalidArgument
                        if e.message().contains("Failed to parse signing session id") =>
                    {
                        return Err(Error::FailedToParseSigningSessionId)
                    }
                    tonic::Code::Internal if e.message().contains("Failed to get to sign") => {
                        return Err(Error::FailedToGetToSign)
                    }
                    tonic::Code::Internal if e.message().contains("Failed to serialize psbt") => {
                        return Err(Error::FailedToSerializePsbt)
                    }
                    _ => return Err(Error::InternalGrpc),
                }
            }
        };
        Ok(package)
    }

    async fn finalize_signing(
        &mut self,
        signing_session_id: Vec<u8>,
    ) -> Result<FinalizeSigningResponse, Error> {
        let finalized_signing = match self
            .btc_client
            .finalize_signing(client::FinalizeSigningRequest { signing_session_id })
            .await
        {
            Ok(finalized_signing) => finalized_signing,
            Err(e) => {
                let e = e.to_tonic_status();
                match e.code() {
                    tonic::Code::InvalidArgument
                        if e.message().contains("Failed to parse signing session id") =>
                    {
                        return Err(Error::FailedToParseSigningSessionId)
                    }
                    tonic::Code::Internal if e.message().contains("Failed to finalize signing") => {
                        return Err(Error::FailedToFinalizeSigning)
                    }
                    tonic::Code::Internal if e.message().contains("Failed to serialize psbt") => {
                        return Err(Error::FailedToSerializePsbt)
                    }
                    _ => return Err(Error::InternalGrpc),
                }
            }
        };
        Ok(finalized_signing)
    }

    async fn abort_signing(&mut self) -> Result<(), Error> {
        match self.btc_client.abort_signing(Empty {}).await {
            Ok(_) => {}
            Err(e) => {
                let e = e.to_tonic_status();
                match e.code() {
                    tonic::Code::Internal if e.message().contains("missing key package") => {
                        return Err(Error::MissingKeyPackage)
                    }
                    _ => return Err(Error::InternalGrpc),
                }
            }
        };
        Ok(())
    }

    pub(crate) async fn get_all_peers_handle(&self) -> Result<HashMap<PeerId, PeerData>, Error> {
        // get all frost peers connections
        let (peers_connections_sender, peers_connections_receiver) =
            tokio::sync::oneshot::channel::<HashMap<PeerId, PeerData>>();
        self.frost_handle
            .send_command(FrostCommand::GetAllConnectedPeers(peers_connections_sender));
        match peers_connections_receiver.await {
            Ok(connected_peers) => Ok(connected_peers),
            Err(e) => {
                error!(target: "consensus::authority::signing", "Failed to get frost peers connections {:?}", e);
                Err(Error::FailedToGetConnectedPeersHandles)
            }
        }
    }

    /// Gets the current federation coordinator. Returns None if it is us, otherwise Some if someone
    /// else is
    pub(crate) async fn get_coordinator(&self) -> Result<Option<(PeerData, u64)>, Error> {
        // check if we are in turn
        let is_inturn = is_inturn(
            self.frost_config.authorities.len() as u64,
            self.frost_config.authority_index as u64,
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
                );
                let current_inturn_authority_frost_identifier =
                    peer_id_to_identifier(current_inturn_authority_index.try_into().unwrap());
                let coord = all_connected_frost_peers.iter().find_map(|(_peer_id, peer_data)| {
                    if peer_data.frost_identifier.as_ref().cloned().unwrap() ==
                        current_inturn_authority_frost_identifier
                    {
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
        is_inturn(
            self.frost_config.authorities.len() as u64,
            self.frost_config.authority_index as u64,
        )
    }

    /// Returns the inturn time data or a given coordinator index. If None, our authority index is
    /// being used
    pub(crate) fn get_inturn_interval_for_coordinator(
        &self,
        coordinator_index: Option<u64>,
    ) -> CoordinatorInterval {
        get_in_turn_interval(
            self.frost_config.authorities.len() as u64,
            coordinator_index.unwrap_or_else(|| self.frost_config.authority_index as u64),
            unix_timestamp(),
        )
    }

    pub(crate) async fn gossip_to_peers(
        &mut self,
        signing_package: SigningPackage,
        frost_identifier: Vec<u8>,
        response_type: SigningEventResponseType,
    ) -> Result<(), Error> {
        let SigningPackage { identifier: _, signing_session_id, psbt } = signing_package;

        let fut = || async {
            // get all connected peers
            let connected_peers = self.get_all_peers_handle().await?;

            // Broadcast signing round 2 package to all peers (excluding ourselves)
            for (_peer_id, connected_peer) in connected_peers.iter() {
                if connected_peer.frost_identifier.as_ref().cloned().unwrap() !=
                    self.personal_frost_identifier
                {
                    let resp = PeerMessageResponse::Signing(SigningResponse {
                        response_type,
                        identifier: frost_identifier.clone(),
                        signing_session_id: signing_session_id.clone(),
                        psbt: psbt.clone(),
                    });
                    if let Some(peer_commands_tx) = connected_peer.peer_commands_tx.as_ref() {
                        peer_commands_tx
                            .send(FrostPeerCommand::PeerMessage(resp))
                            .map_err(Error::Send)?;
                    }
                }
            }
            Ok(())
        };

        retry_exec(fut, 3, Duration::from_secs(1)).await
    }

    // ====================================== 1 =========================================
    // Coordinator initiates a new signing session
    pub(crate) async fn initate_signing_session(
        &mut self,
        signing_session_id: Vec<u8>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let session_id = parse_signing_session_id(&signing_session_id)?;

        // the coordinator is always expected to be us in this case, i.e. None
        let coordinator = self.get_coordinator().await?;
        if coordinator.is_some() {
            error!(target: "consensus::authority::signing::initate_signing_session", "A non-coordinator is trying to (re)initiate a signing process!");
            return Ok(());
        }
        let (start, end, time_passed, time_remaining) =
            self.get_inturn_interval_for_coordinator(None);

        // try to find the signing session in cache in case it was re-triggered by the same
        // coordinator
        match self.get_signing_session(session_id).await {
            Some(signing_session) => {
                // session was previously already registered, maybe it got retriggered
                // check if it was the same coordinator
                // check if it is still valid time-wise
                if (signing_session.coordinator_index != self.frost_config.authority_index as u64) ||
                    time_remaining < time_passed
                {
                    // session is no longer valid, remove it from cache and return
                    self.remove_signing_session(session_id).await;
                    return Ok(());
                } else {
                    // still valid session, reinitiate it and continue
                    self.update_signing_state(session_id, SigningState::Initial).await;
                }
            }
            None => {
                // no previous session, insert a new one and continue
                self.insert_new_signing_session(
                    session_id,
                    self.frost_config.authority_index as u64,
                    Some(psbt.clone()),
                    SigningState::Initial,
                    start,
                    end,
                )
                .await;
            }
        }

        info!(target: "consensus::authority::signing::initate_signing_session", "restarting signing session with id {:?}", session_id);

        // As the cord we generate round 1 nonces and save them
        // then we send the psbt to other peers
        let signing_round1_package =
            self.get_round1_signing_package(signing_session_id.clone(), psbt).await?;
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
    /// Note: since this is a request idenfitier has no use
    pub(crate) async fn signer_process_round1(
        &mut self,
        _identifier: Vec<u8>,
        signing_session_id: Vec<u8>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let session_id = parse_signing_session_id(&signing_session_id)?;

        // return if we are a coordinator
        if self.is_coordinator() {
            return Ok(());
        }

        // get coordinator
        let coordinator = self.get_coordinator().await?;

        // get coordinator id
        let coordinator_id =
            coordinator.as_ref().map(|(_, authority_index)| *authority_index).unwrap_or_default();

        // get coordinator time interval validity
        let (start, end, _time_passed, _time_remaining) =
            self.get_inturn_interval_for_coordinator(Some(coordinator_id));

        // no already existing signing session found
        if !self.signing_session_exists(session_id).await {
            // abort any previous session
            self.abort_signing().await?;

            // insert a new signing session
            self.insert_new_signing_session(
                session_id,
                coordinator_id,
                None,
                SigningState::Round1,
                start,
                end,
            )
            .await;
        } else {
            // check session is still valid
            let (_prev_validity_from, prev_validity_to) = self
                .get_signing_session(session_id)
                .await
                .as_ref()
                .map(|s| (s.validity_from, s.validity_to))
                .unwrap_or_default();

            // abort this previous session as it is outdated
            if unix_timestamp() >= prev_validity_to {
                self.abort_signing().await?;
                self.update_signing_state(session_id, SigningState::Failed).await;
                return Ok(());
            }

            // session exists and is valid, return if we are not in round 2
            if self.is_round2_state(&session_id).await {
                return Ok(());
            } else {
                // current session is being re-triggered, so set to round1
                self.update_signing_state(session_id, SigningState::Round1).await;
            }
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

        let (coordinator_frost_id, coordinator_sender, _) = coordinator.unwrap();
        info!(target: "consensus::authority::signing::signer_process_round1", "coordinator id {:?}", coordinator_frost_id);
        // Broadcast signing round 1 to the coordinator
        if coordinator_peer_data.frost_identifier.as_ref().cloned().unwrap() !=
            self.personal_frost_identifier
        {
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
                        if let Some(sender) = sender.as_ref() {
                            return sender
                                .send(FrostPeerCommand::PeerMessage(message))
                                .map_err(Error::Send)
                        }
                        Ok(())
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
        signing_session_id: Vec<u8>,
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
        if let Err(e) = self
            .new_round1_signing_package(identifier.clone(), signing_session_id.clone(), psbt)
            .await
        {
            error!(target: "consensus::authority::signing::coordinator_process_round1","Error adding round 1 signing package {:?}", e);
            return Ok(());
        }

        // try to generate signing package
        if let Ok(to_sign_payload) = self.get_to_sign_package(signing_session_id.clone()).await {
            // we should add the cord partial sig
            let cord_round2 = self
                .get_round2_signing_package(
                    signing_session_id.clone(),
                    to_sign_payload.psbt.clone(),
                )
                .await?;
            self.new_round2_signing_package(
                self.personal_frost_identifier.serialize().to_vec(),
                signing_session_id,
                cord_round2.psbt,
            )
            .await?;

            // if we can, we go to round 2
            self.update_signing_state(session_id, SigningState::Round2).await;
            // if ok, send to all peers
            if let Err(e) = self
                .gossip_to_peers(
                    to_sign_payload.clone(),
                    identifier,
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
    /// Note that since this is a request idenfitier has no use
    pub(crate) async fn signer_process_round2(
        &mut self,
        _identifier: Vec<u8>,
        signing_session_id: Vec<u8>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let session_id = parse_signing_session_id(&signing_session_id)?;

        // return if we are a coordinator
        if self.is_coordinator() {
            warn!(
                target: "consensus::authority::signing::signer_process_round2",
                "we are the coordinator",
            );
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

        // get coordinator
        let coordinator = self.get_coordinator().await?;
        // if none, we are coordinator, if some, someone else is
        if coordinator.is_none() {
            warn!(target: "consensus::authority::signing::signer_process_round2", "No coordinator found");
            return Ok(());
        }

        let (coordinator_peer_data, _coordinator_authority_index) = coordinator.unwrap();

        // Broadcast signing round 2 to the coordinator
        if coordinator_peer_data.frost_identifier.as_ref().cloned().unwrap() !=
            self.personal_frost_identifier
        {
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
                        if let Some(sender) = sender.as_ref() {
                            return sender
                                .send(FrostPeerCommand::PeerMessage(message))
                                .map_err(Error::Send)
                        }
                        Ok(())
                    }
                },
                3,
                Duration::from_secs(1),
            )
            .await?
        }

        Ok(())
    }

    /// A coordinator processes round 2 signing packages
    pub(crate) async fn coordinator_process_round2(
        &mut self,
        identifier: Vec<u8>,
        signing_session_id: Vec<u8>,
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
                " we are not the coordinator",
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
            info!(target: "consensus::authority::signing::coordinator_process_round2", "identifier is us");
            return Ok(());
        }

        // add the transmitted round 2 package data
        if let Err(e) = self
            .new_round2_signing_package(identifier.clone(), signing_session_id.clone(), psbt)
            .await
        {
            error!(target: "consensus::authority::signing::coordinator_process_round2", "Error adding round 2 signing package {:?}", e);
            self.update_signing_state(session_id, SigningState::Failed).await;
            return Err(e);
        }
        info!(target: "consensus::authority::signing::coordinator_process_round2", "round 2 added");

        // try to finalize the signing
        if let Ok(sign_payload) = self.finalize_signing(signing_session_id.clone()).await {
            if let Err(e) = self.frost_task_tx.send(FrostNotificationMessage::FinalizedSignature(
                FrostNotification { signing_session_id, psbt: sign_payload.psbt },
            )) {
                error!(target: "consensus::authority::signing::coordinator_process_round2", "Error sending finalized signature {:?}", e);
            }
            self.update_signing_state(session_id, SigningState::Finalized).await
        }

        Ok(())
    }

    /// Handles an errored singing process
    pub(crate) async fn handle_errored_signing_process(
        &mut self,
        signing_session_id: Vec<u8>,
    ) -> Result<(), Error> {
        // parse the session id
        let session_id = parse_signing_session_id(&signing_session_id)?;

        // make sure we are in a failed state
        if !self.is_failed_state(&session_id).await {
            warn!(target: "consensus::authority::signing::handle_errored_signing_process",
                "Session id {:?} has not failed",
                &session_id
            );
            return Ok(());
        }

        // only if we are coordinator AND we are in a failed state, then we can restart the signing
        // process provided there is enough time left
        if self.is_coordinator() {
            // check there is sufficient time remaining to retry the signing request
            let (_start, _end, time_passed, time_remaining) =
                self.get_inturn_interval_for_coordinator(None);

            // check if we can repeat the session, if not abort and return
            if time_remaining < time_passed {
                self.abort_signing().await?;
                warn!(target: "consensus::authority::signing::handle_errored_signing_process", "Insuficient time remaining to retry the signing request");
                return Ok(());
            }

            // get the signing session, if not found abort and return
            let signing_session = self.get_signing_session(session_id).await;
            if signing_session.is_none() {
                self.abort_signing().await?;
                error!(target: "consensus::authority::signing::handle_errored_signing_process", "Could not find the the signing session for session id = {:?}", session_id);
                return Ok(());
            }

            // if all good, re-initiate the signing rounds
            if let Err(e) = self.frost_task_tx.send(FrostNotificationMessage::InitiateSigning(
                FrostNotification {
                    signing_session_id,
                    psbt: signing_session
                        .unwrap()
                        .original_psbt
                        .expect("Original psbt to be valid and present"),
                },
            )) {
                error!(target: "consensus::authority::signing::handle_errored_signing_process", "Error trying to re-initialize failed signing session with session id {:?}. Error = {:?}", &session_id, e);
            }
        }

        Ok(())
    }
}
