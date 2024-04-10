use crate::{
    extended_client::BtcServerExtendedClient,
    frost_task::{FrostNotification, FrostNotificationMessage},
    utils::{deserialize_frost_peer_id, parse_signing_session_id, FrostParseError},
    Storage,
};
use client::{
    FinalizeSigningResponse, Output, Round1SigningPackage, Round2SigningPackage, SignPayload,
};
use frost_secp256k1_tr as frost;
use reth_botanix_lib::peg_contract::PegoutData;
use reth_consensus_common::utils::{current_inturn_index, is_inturn};
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_network::frost::{
    manager::{peer_id_to_identifier, FrostCommand, FrostConfig, FrostHandle},
    FrostPeerCommand, PeerMessageResponse, SigningEventResponseType, SigningResponse,
};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use std::collections::HashMap;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info, warn};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Unknwon internal error")]
    InternalGrpc,
    #[error("Failed to get connected peers handles")]
    FailedToGetConnectedPeersHandles,
    #[error("Invalid frost peer id")]
    InvalidFrostPeerId,
    #[error("Failed to get frost coordinator")]
    FailedToGetFrostCoordinator,
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
    #[error("invalid signing session id")]
    InvalidSigningSessionId,
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
    /// Returns true if the signing state machine is in a running state
    pub(crate) fn is_running(&self) -> bool {
        match self {
            SigningState::Initial | SigningState::Finalized | SigningState::Failed => false,
            _ => true,
        }
    }
    /// Returns true if we are in round 1 of the signing
    pub(crate) fn is_round1(&self) -> bool {
        matches!(self, SigningState::Round1)
    }
    /// Returns true if we are in round 2 of signing
    pub(crate) fn is_round2(&self) -> bool {
        matches!(self, SigningState::Round2)
    }
    /// Returns true if we are in a finalized signing state
    pub(crate) fn is_finalized(&self) -> bool {
        matches!(self, SigningState::Finalized)
    }

    /// Returns true if the signing has failed
    pub(crate) fn has_failed(&self) -> bool {
        matches!(self, SigningState::Failed)
    }
}

/// A state machine for transitioning between different signing states
#[derive(Debug)]
pub(crate) struct SigningStateMachine<Client> {
    btc_client: BtcServerExtendedClient,
    storage: Storage<Client>,
    frost_handle: FrostHandle,
    signing_states: HashMap<[u8; 32], SigningState>,
    personal_frost_identifier: frost::Identifier,
    frost_config: FrostConfig,
    frost_task_tx: UnboundedSender<FrostNotificationMessage>,
}

impl<Client> SigningStateMachine<Client>
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
        btc_client: BtcServerExtendedClient,
        storage: Storage<Client>,
        frost_handle: FrostHandle,
        frost_config: FrostConfig,
        frost_task_tx: UnboundedSender<FrostNotificationMessage>,
    ) -> Self {
        let personal_frost_identifier: frost::Identifier =
            peer_id_to_identifier(frost_config.authority_index as u16);
        info!(
            "Frost identifier used: {:?} - {:?}",
            frost_config.authority_index, personal_frost_identifier
        );
        Self {
            btc_client,
            storage,
            frost_handle,
            signing_states: HashMap::default(),
            personal_frost_identifier,
            frost_config,
            frost_task_tx,
        }
    }

    /// Resets the state machine to its initial state
    #[allow(dead_code)]
    pub(crate) fn reset(self) -> Self {
        Self {
            btc_client: self.btc_client,
            storage: self.storage,
            frost_handle: self.frost_handle,
            signing_states: self.signing_states,
            personal_frost_identifier: self.personal_frost_identifier,
            frost_config: self.frost_config,
            frost_task_tx: self.frost_task_tx,
        }
    }

    /// Returns the state machine state
    pub(crate) fn get_or_insert_signing_state(
        &mut self,
        session_id: [u8; 32],
        signing_state: SigningState,
    ) -> SigningState {
        *self.signing_states.entry(session_id).or_insert(signing_state)
    }

    /// Inserts a signing state into the state machine
    pub(crate) fn insert_signing_state(
        &mut self,
        session_id: [u8; 32],
        signing_state: SigningState,
    ) {
        self.signing_states.insert(session_id, signing_state);
    }
}

impl<Client> SigningStateMachine<Client>
where
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
    ) -> Result<Round1SigningPackage, Error> {
        let round1_payload = self
            .btc_client
            .get_round1_signing_package(client::Round1SigningPackageRequest {
                psbt,
                signing_session_id,
            })
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
    ) -> Result<Round2SigningPackage, Error> {
        let round2_payload = self
            .btc_client
            .get_round2_signing_package(client::SignPayload { psbt, signing_session_id })
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
            .new_round1_signing_package(client::Round1SigningPackage {
                identifier,
                psbt,
                signing_session_id,
            })
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
            .new_round2_signing_package(client::Round2SigningPackage {
                identifier,
                psbt,
                signing_session_id,
            })
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
    ) -> Result<SignPayload, Error> {
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

    pub(crate) async fn get_coordinator(
        &self,
    ) -> Result<Option<(frost::Identifier, UnboundedSender<FrostPeerCommand>)>, Error> {
        // check if we are in turn
        let is_inturn = is_inturn(
            self.frost_config.authorities.len() as u64,
            self.frost_config.authority_index as u64,
        );
        match is_inturn {
            true => {
                // if we are inturn, return None to avoid sending messages to ourselves.
                return Ok(None);
            }
            false => {
                // if we are not inturn, find the coordinator in the list of peers
                let all_connected_frost_peers = self.get_all_peers_handle().await?;
                let current_inturn_authority_index =
                    current_inturn_index(self.frost_config.authorities.len() as u64);
                let current_inturn_authority_frost_identifier =
                    peer_id_to_identifier(current_inturn_authority_index.try_into().unwrap());
                let sender_channel = all_connected_frost_peers
                    .get(&current_inturn_authority_frost_identifier)
                    .cloned();
                return Ok(Some(current_inturn_authority_frost_identifier).zip(sender_channel));
            }
        }
    }

    pub(crate) fn is_coordinator(&self) -> bool {
        is_inturn(
            self.frost_config.authorities.len() as u64,
            self.frost_config.authority_index as u64,
        )
    }

    // ====================================== 1 =========================================
    // Coordinator initiates a new signing session
    pub(crate) async fn initate_signing_session(
        &mut self,
        signing_session_id: Vec<u8>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let session_id = parse_signing_session_id(&signing_session_id)?;
        let _ = self.insert_signing_state(session_id, SigningState::Initial);
        info!(">>>>>>>>>>> [START NEW SIGNING SESSION]");

        // As the cord we generate round 1 nonces and save them
        // then we send the psbt to other peers
        let signing_round1_package =
            self.get_round1_signing_package(signing_session_id.clone(), psbt.clone()).await?;
        self.new_round1_signing_package(
            self.personal_frost_identifier.serialize().to_vec(),
            signing_session_id,
            signing_round1_package.clone().psbt,
        )
        .await?;

        // send to all other peers
        let _ = self.insert_signing_state(session_id, SigningState::Round1);
        if let Err(e) = self.gossip_round1_to_peers(signing_round1_package).await {
            error!("Error gossiping round 1 to peers {:?}", e);
            let _ = self.insert_signing_state(session_id, SigningState::Failed);
            return Err(e);
        }
        Ok(())
    }

    pub(crate) async fn gossip_round1_to_peers(
        &mut self,
        round1_signing_package: Round1SigningPackage,
    ) -> Result<(), Error> {
        // get all connected peers
        let connected_peers = self.get_all_peers_handle().await?;
        info!(">>>>>>>>>>> [GOSSIP_ROUND1] number of peers connected: {:?}", connected_peers.len());

        let Round1SigningPackage { signing_session_id, psbt, identifier } = round1_signing_package;

        // Broadcast signing round 1 package to all peers (excluding ourselves)
        connected_peers.iter().for_each(|(frost_id, sender)| {
            if *frost_id != self.personal_frost_identifier {
                let resp = PeerMessageResponse::Signing(SigningResponse {
                    response_type: SigningEventResponseType::SignerRound1SigningPackage,
                    identifier: identifier.clone(),
                    signing_session_id: signing_session_id.clone(),
                    psbt: psbt.clone(),
                });
                let _ = sender.send(FrostPeerCommand::PeerMessage(resp)); // TODO: map to error ?
            }
        });
        Ok(())
    }

    /// A signer processes round 1 signing packages
    pub(crate) async fn signer_process_round1(
        &mut self,
        identifier: Vec<u8>,
        signing_session_id: Vec<u8>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let session_id = parse_signing_session_id(&signing_session_id)?;

        let is_round2 =
            self.signing_states.get(&session_id).map(|state| state.is_round2()).unwrap_or_default();

        // return if we are not peer and not in round 1
        if is_round2 {
            return Ok(());
        } else {
            self.insert_signing_state(session_id, SigningState::Round1);
        }

        info!(
            ">>>>>>>>>>> [PROCESS_ROUND1] identifiers {:?} {:?}",
            self.personal_frost_identifier,
            deserialize_frost_peer_id(identifier.clone())?
        );

        // return if the sending identifier is us
        if self.personal_frost_identifier == deserialize_frost_peer_id(identifier.clone())? {
            return Ok(());
        }

        // add the transmitted round 1 package data
        let signing_package_round1 =
            match self.get_round1_signing_package(signing_session_id, psbt).await {
                Ok(signing_package_round1) => signing_package_round1,
                Err(e) => {
                    error!("Error adding round 2 signing package {:?}", e);
                    let _ = self.insert_signing_state(session_id, SigningState::Failed);
                    return Err(e);
                }
            };
        // Update signing state
        self.insert_signing_state(session_id, SigningState::Round2);

        // get coordinator
        let coordinator = self.get_coordinator().await?;
        // if none, we are coordinator ?
        if coordinator.is_none() {
            return Ok(());
        }

        let (coordinator_frost_id, coordinator_sender) = coordinator.unwrap();
        info!(">>>>>>>>>>> [PROCESS_ROUND1] coordinator {:?}", coordinator_frost_id);
        // Broadcast signing round 1 to the coordinator
        if coordinator_frost_id != self.personal_frost_identifier {
            // TODO fix unwrap
            let resp = PeerMessageResponse::Signing(SigningResponse {
                response_type: SigningEventResponseType::CoordinatorRound1SigningPackage,
                identifier: signing_package_round1.identifier.clone(),
                signing_session_id: signing_package_round1.signing_session_id.clone(),
                psbt: signing_package_round1.psbt.clone(),
            });
            let _ = coordinator_sender.send(FrostPeerCommand::PeerMessage(resp));
            // TODO: map error
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
        info!(">>>>>>>>>>> [COORDINATOR PROCESS_ROUND1] session id {:?}", session_id);
        info!(">>>>>>>>>>> [COORDINATOR PROCESS_ROUND1] signing states {:?}", self.signing_states);
        let is_round1 =
            self.signing_states.get(&session_id).map(|state| state.is_round1()).unwrap_or_default();
        // return if we are not in round 1 or not a coordinator
        if !is_round1 {
            warn!(">>>>>>>>>>> [COORDINATOR PROCESS_ROUND1] is_round1 {:?}", is_round1);
            return Ok(());
        }
        if !self.is_coordinator() {
            warn!(
                ">>>>>>>>>>> [COORDINATOR PROCESS_ROUND1] we are not the coordinator {:?}",
                self.is_coordinator()
            );
            return Ok(());
        }

        info!(
            ">>>>>>>>>>> [COORDINATOR PROCESS_ROUND1] identifiers my peer id: {:?}, other peerid {:?}",
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
            error!("[COORDINATOR PROCESS_ROUND1] Error adding round 1 signing package {:?}", e);
            return Ok(());
            // let _ = self.get_or_insert_signing_state(session_id, SigningState::Failed);
            // return Err(e);
        }

        // try to generate signing package
        if let Ok(sign_payload) = self.get_to_sign_package(signing_session_id.clone()).await {
            // we should add the cord partial sig
            let cord_round2 = self
                .get_round2_signing_package(signing_session_id.clone(), sign_payload.psbt.clone())
                .await?;
            self.new_round2_signing_package(
                self.personal_frost_identifier.serialize().to_vec(),
                signing_session_id,
                cord_round2.psbt,
            )
            .await?;

            // if we can, we go to round 2
            let _ = self.insert_signing_state(session_id, SigningState::Round2);
            // if ok, send to all peers
            if let Err(e) = self.gossip_round2_to_peers(sign_payload.clone(), identifier).await {
                error!("[COORDINATOR PROCESS_ROUND1] Error gossiping round 2 to peers {:?}", e);
                let _ = self.insert_signing_state(session_id, SigningState::Failed);
                return Err(e);
            }
            info!(
                ">>>>>>>>>>> [COORDINATOR PROCESS_ROUND1] to sign payload send to signers {:?}",
                sign_payload
            );
        }

        Ok(())
    }

    // ====================================== 2 =========================================

    pub(crate) async fn gossip_round2_to_peers(
        &mut self,
        sign_payload: SignPayload,
        frost_identifier: Vec<u8>,
    ) -> Result<(), Error> {
        // get all connected peers
        let connected_peers = self.get_all_peers_handle().await?;

        let SignPayload { signing_session_id, psbt } = sign_payload;

        // Broadcast signing round 2 package to all peers (excluding ourselves)
        connected_peers.iter().for_each(|(frost_id, sender)| {
            if *frost_id != self.personal_frost_identifier {
                let resp = PeerMessageResponse::Signing(SigningResponse {
                    response_type: SigningEventResponseType::SignerRound2SigningPackage,
                    identifier: frost_identifier.clone(),
                    signing_session_id: signing_session_id.clone(),
                    psbt: psbt.clone(),
                });
                let _ = sender.send(FrostPeerCommand::PeerMessage(resp)); // TODO: map to error ?
            }
        });
        Ok(())
    }

    /// A signer processes round 2 signing request
    pub(crate) async fn signer_process_round2(
        &mut self,
        identifier: Vec<u8>,
        signing_session_id: Vec<u8>,
        psbt: Vec<u8>,
    ) -> Result<(), Error> {
        let session_id = parse_signing_session_id(&signing_session_id)?;

        let is_round2 =
            self.signing_states.get(&session_id).map(|state| state.is_round2()).unwrap_or_default();

        // return if we are not peer and not in round 2
        if !is_round2 {
            warn!(">>>>>>>>>>> [SIGNER PROCESS_ROUND2] is_round2 {:?}", is_round2);
            return Ok(());
        }

        if self.is_coordinator() {
            warn!(
                ">>>>>>>>>>> [SIGNER PROCESS_ROUND2] we are the coordinator {:?}",
                self.is_coordinator()
            );
            return Ok(());
        }

        info!(
            ">>>>>>>>>>> [SIGNER PROCESS_ROUND2] identifiers {:?} {:?}",
            self.personal_frost_identifier,
            deserialize_frost_peer_id(identifier.clone())?
        );

        // return if the sending identifier is us
        if self.personal_frost_identifier == deserialize_frost_peer_id(identifier.clone())? {
            return Ok(());
        }

        // add the transmitted round 2 package data
        let signing_package_round2 =
            match self.get_round2_signing_package(signing_session_id, psbt).await {
                Ok(signing_package_round2) => signing_package_round2,
                Err(e) => {
                    error!("Error adding round 2 signing package {:?}", e);
                    let _ = self.insert_signing_state(session_id, SigningState::Failed);
                    return Err(e);
                }
            };
        info!(
            ">>>>>>>>>>> [SIGNER PROCESS_ROUND2] signing_package_round2 {:?}",
            signing_package_round2
        );
        // get coordinator
        let coordinator = self.get_coordinator().await?;
        // if none, we are coordinator ?
        if coordinator.is_none() {
            warn!(">>>>>>>>>>> [SIGNER PROCESS_ROUND2] No coordinator found");
            return Ok(());
        }

        let (coordinator_frost_id, coordinator_sender) = coordinator.unwrap();

        // Broadcast signing round 2 to the coordinator
        if coordinator_frost_id != self.personal_frost_identifier {
            // fix unwrap
            let resp = PeerMessageResponse::Signing(SigningResponse {
                response_type: SigningEventResponseType::CoordinatorRound2SigningPackage,
                identifier: signing_package_round2.identifier.clone(),
                signing_session_id: signing_package_round2.signing_session_id.clone(),
                psbt: signing_package_round2.psbt.clone(),
            });
            let _ = coordinator_sender.send(FrostPeerCommand::PeerMessage(resp));
            // TODO: map error
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

        let is_round2 =
            self.signing_states.get(&session_id).map(|state| state.is_round2()).unwrap_or_default();

        // return if we are not in round 2 or not a coordinator
        if !is_round2 {
            warn!(">>>>>>>>>>> [COORDINATOR PROCESS_ROUND2] is_round2 {:?}", is_round2);
            return Ok(());
        }

        if !self.is_coordinator() {
            warn!(
                ">>>>>>>>>>> [COORDINATOR PROCESS_ROUND2] we are not the coordinator {:?}",
                self.is_coordinator()
            );
            return Ok(());
        }

        info!(
            ">>>>>>>>>>> [PROCESS_ROUND2 Coordinator] identifier {:?} {:?}",
            self.personal_frost_identifier,
            deserialize_frost_peer_id(identifier.clone())?
        );

        // return if the sending identifier is us
        if self.personal_frost_identifier == deserialize_frost_peer_id(identifier.clone())? {
            info!(">>>>>>>>>>> [PROCESS_ROUND2 Coordinator] identifier is us");
            return Ok(());
        }

        // add the transmitted round 2 package data
        if let Err(e) = self
            .new_round2_signing_package(identifier.clone(), signing_session_id.clone(), psbt)
            .await
        {
            error!(">>>>>>>>>>> [PROCESS_ROUND2 Coordinator] Error adding round 2 signing package {:?}", e);
            let _ = self.insert_signing_state(session_id, SigningState::Failed);
            return Err(e);
        }
        info!(">>>>>>>>>>> [PROCESS_ROUND2 Coordinator] round 2 added");

        // try to finalize the signing
        if let Ok(sign_payload) = self.finalize_signing(signing_session_id.clone()).await {
            if let Err(e) = self.frost_task_tx.send(FrostNotificationMessage::FinalizedSignature(
                FrostNotification { signing_session_id, psbt: sign_payload.psbt },
            )) {
                error!("Error sending finalized signature {:?}", e);
            }
            let _ = self.insert_signing_state(session_id, SigningState::Finalized);
        }

        Ok(())
    }
}
