use client::{DkgPayload, Empty, GetPublicKeyResponse};
use frost_secp256k1_tr as frost;
use reth_network::frost::{
    manager::{peer_id_to_identifier, FrostCommand, FrostConfig, PeerData, ToFrostManager},
    DkgEventResponseType, DkgResponse, FrostPeerCommand, PeerMessageResponse,
};
use reth_rpc_types::PeerId;
use std::{
    collections::{BTreeMap, HashMap},
    str::FromStr,
};
use tokio::sync::mpsc::error::SendError;
use tracing::{error, info, warn};

use crate::{
    extended_client::BtcServerExtendedClient,
    utils::{deserialize_frost_peer_id, FrostParseError},
    Storage,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Requested Key Package already exists")]
    KeyPackageAlreadyExists,
    #[error("Round 1 package is missing")]
    MissingRound1Package,
    #[error("Round 2 package is missing")]
    MissingRound2Package,
    #[error("Failed to get Round 1 packages")]
    FailedToGetRound1Packages,
    #[error("Failed to get Round 2 packages")]
    FailedToGetRound2Packages,
    #[error("Failed to generate Round 1 package")]
    FailedToGenerateRound1Package,
    #[error("Failed to generate Round 2 package")]
    FailedToGenerateRound2Package,
    #[error("Error when serializing round 1 packages")]
    Round1PackageSerialize,
    #[error("Failed to get public key package")]
    FailedToGetPubKeyPackage,
    #[error("Failed to generate public key package")]
    FailedToGeneratePubKeyPackage,
    #[error("Failed to store key package")]
    FailedToStoreKeyPackage,
    #[error("Failed to store public key package")]
    FailedToStorePublicKeyPackage,
    #[error("Failed to perisist pk to db")]
    FailedToPeristPkToDb,
    #[error("Failed to add round 1 package")]
    FailedToAddRound1Package,
    #[error("Failed to add round 2 package")]
    FailedToAddRound2Package,
    #[error("Failed to parse public key package")]
    PublicKeyParse(secp256k1::Error),
    #[error("Unknwon internal error")]
    InternalGrpc,
    #[error("Failed to get connected peers handles")]
    FailedToGetConnectedPeersHandles,
    #[error("Invalid frost peer id")]
    InvalidFrostPeerId,
    #[error("invalid signing session id")]
    InvalidSigningSessionId,
    #[error("missing key package")]
    MissingKeyPackage,
    #[error("Failed to send peer command {0}")]
    Send(SendError<FrostPeerCommand>),
}

impl From<FrostParseError> for Error {
    fn from(value: FrostParseError) -> Self {
        match value {
            FrostParseError::InvalidFrostPeerId => Error::InvalidFrostPeerId,
            FrostParseError::InvalidSigningSessionId => Error::InvalidSigningSessionId,
        }
    }
}

/// Defines the states of the state machine
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DKGState {
    /// The initial dkg state
    Initial,
    /// Round 1 of dkg has been started
    Running,
    /// The dkg state machine has failed
    DkgFailed,
}

impl DKGState {
    /// Returns true if the DKG state machine is in a running state
    pub(crate) fn is_running(&self) -> bool {
        !matches!(self, DKGState::Initial)
    }
}

/// A state machine for transitioning between different DKG states
#[derive(Debug, Clone)]
pub(crate) struct DKGStateMachine<ToFrostMan> {
    btc_client: BtcServerExtendedClient,
    storage: Storage,
    frost_handle: ToFrostMan,
    state: DKGState,
    personal_frost_identifier: frost::Identifier,
    public_key_package: Option<secp256k1::PublicKey>,
    frost_config: FrostConfig,
    // coordiantor only fields
    // Key frost id, values are round 1 and round 2 packages respectively
    round1_packages: BTreeMap<Vec<u8>, Vec<u8>>,
    round2_packages: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl<ToFrostMan> DKGStateMachine<ToFrostMan>
where
    ToFrostMan: ToFrostManager + Clone,
{
    /// Constructs a new state machine with the given params
    pub(crate) fn new(
        btc_client: BtcServerExtendedClient,
        storage: Storage,
        frost_handle: ToFrostMan,
        frost_config: FrostConfig,
    ) -> Self {
        let personal_frost_identifier: frost::Identifier =
            peer_id_to_identifier(frost_config.authority_index as u16);
        Self {
            btc_client,
            storage,
            frost_handle,
            state: DKGState::Initial,
            personal_frost_identifier,
            public_key_package: None,
            frost_config,
            round1_packages: BTreeMap::new(),
            round2_packages: BTreeMap::new(),
        }
    }

    /// Resets the state machine to its initial state
    #[allow(dead_code)]
    pub(crate) fn reset(self) -> Self {
        Self {
            btc_client: self.btc_client,
            storage: self.storage,
            frost_handle: self.frost_handle,
            state: DKGState::Initial,
            personal_frost_identifier: self.personal_frost_identifier,
            public_key_package: None,
            frost_config: self.frost_config,
            round1_packages: BTreeMap::new(),
            round2_packages: BTreeMap::new(),
        }
    }

    /// Returns the public key package
    #[allow(dead_code)]
    pub(crate) fn get_public_key_package(&self) -> Option<secp256k1::PublicKey> {
        self.public_key_package
    }

    /// Returns the state machine state
    pub(crate) fn get_dkg_state(&self) -> DKGState {
        self.state
    }
}

impl<ToFrostMan> DKGStateMachine<ToFrostMan>
where
    ToFrostMan: ToFrostManager + Clone,
{
    async fn get_round1_dkg_package(&mut self) -> Result<DkgPayload, Error> {
        let round1_payload = self.btc_client.get_round1_dkg_package(client::Empty {}).await;

        let round1_payload = match round1_payload {
            Ok(round1_payload) => round1_payload,
            Err(e) => {
                let e: tonic::Status = e.to_tonic_status();
                match e.code() {
                    tonic::Code::AlreadyExists
                        if e.message().contains("already have key package") =>
                    {
                        return Err(Error::KeyPackageAlreadyExists)
                    }
                    tonic::Code::Internal
                        if e.message().contains("Failed to serialize round 1 dkg") =>
                    {
                        return Err(Error::Round1PackageSerialize)
                    }
                    tonic::Code::Internal if e.message().contains("Missing round1 dkg package") => {
                        return Err(Error::MissingRound1Package)
                    }
                    tonic::Code::Internal
                        if e.message().contains("Failed to generate round 1 dkg") =>
                    {
                        return Err(Error::FailedToGenerateRound1Package)
                    }
                    _ => return Err(Error::InternalGrpc),
                }
            }
        };
        Ok(round1_payload)
    }

    async fn get_round2_dkg_package(&mut self) -> Result<DkgPayload, Error> {
        let round2_payload = self.btc_client.get_round2_dkg_package(client::Empty {}).await;

        let round2_payload = match round2_payload {
            Ok(round2_payload) => round2_payload,
            Err(e) => {
                let e: tonic::Status = e.to_tonic_status();
                match e.code() {
                    tonic::Code::AlreadyExists
                        if e.message().contains("already have key package") =>
                    {
                        return Err(Error::KeyPackageAlreadyExists)
                    }
                    tonic::Code::Internal if e.message().contains("Missing round1 dkg package") => {
                        return Err(Error::MissingRound1Package)
                    }
                    tonic::Code::Internal
                        if e.message().contains("Failed to get round2 dkg package") =>
                    {
                        return Err(Error::FailedToGetRound2Packages)
                    }
                    tonic::Code::InvalidArgument
                        if e.message().contains("Failed to generate round 2 dkg") =>
                    {
                        return Err(Error::FailedToGenerateRound2Package)
                    }
                    _ => return Err(Error::InternalGrpc),
                }
            }
        };
        Ok(round2_payload)
    }

    pub(crate) async fn get_public_key(&mut self) -> Result<GetPublicKeyResponse, Error> {
        let round3_payload = self.btc_client.get_public_key(Empty {}).await;
        let round3_payload = match round3_payload {
            Ok(round3_payload) => round3_payload,
            Err(e) => {
                let e: tonic::Status = e.to_tonic_status();
                match e.code() {
                    tonic::Code::Internal
                        if e.message().contains("Failed to get public key package") =>
                    {
                        return Err(Error::FailedToGetPubKeyPackage)
                    }
                    tonic::Code::Internal if e.message().contains("missing key package") => {
                        return Err(Error::MissingKeyPackage)
                    }
                    tonic::Code::Internal
                        if e.message().contains("Failed to get round1 dkg package") =>
                    {
                        return Err(Error::FailedToGetRound1Packages)
                    }
                    tonic::Code::Internal
                        if e.message().contains("Failed to get round2 dkg package") =>
                    {
                        return Err(Error::FailedToGetRound2Packages)
                    }
                    tonic::Code::Internal
                        if e.message().contains("Failed to generate public key package") =>
                    {
                        return Err(Error::FailedToGeneratePubKeyPackage)
                    }
                    tonic::Code::Internal
                        if e.message().contains("Failed to store key package") =>
                    {
                        return Err(Error::FailedToStoreKeyPackage)
                    }
                    tonic::Code::Internal
                        if e.message().contains("Failed to store public key package") =>
                    {
                        return Err(Error::FailedToStorePublicKeyPackage)
                    }
                    tonic::Code::Internal
                        if e.message().contains("Failed to persist pk to database") =>
                    {
                        return Err(Error::FailedToPeristPkToDb)
                    }
                    tonic::Code::Internal if e.message().contains("Missing round2 dkg package") => {
                        return Err(Error::MissingRound2Package)
                    }
                    _ => return Err(Error::InternalGrpc),
                }
            }
        };
        Ok(round3_payload)
    }

    async fn add_round1_dkg_package(
        &mut self,
        identifier: Vec<u8>,
        payload: Vec<u8>,
    ) -> Result<(), Error> {
        let req = client::DkgPayload { identifier, payload };
        match self.btc_client.new_round1_dkg_package(req).await {
            Ok(round2_payload) => round2_payload,
            Err(e) => {
                let e: tonic::Status = e.to_tonic_status();
                match e.code() {
                    tonic::Code::AlreadyExists
                        if e.message().contains("already have key package") =>
                    {
                        return Err(Error::KeyPackageAlreadyExists)
                    }
                    tonic::Code::InvalidArgument
                        if e.message().contains("Failed to add round1 dkg") =>
                    {
                        return Err(Error::FailedToAddRound1Package)
                    }
                    _ => return Err(Error::InternalGrpc),
                }
            }
        };
        Ok(())
    }

    async fn add_round2_dkg_package(
        &mut self,
        identifier: Vec<u8>,
        payload: Vec<u8>,
    ) -> Result<(), Error> {
        let req = client::DkgPayload { identifier, payload };
        match self.btc_client.new_round2_dkg_package(req).await {
            Ok(round2_payload) => round2_payload,
            Err(e) => {
                let e: tonic::Status = e.to_tonic_status();
                match e.code() {
                    tonic::Code::AlreadyExists
                        if e.message().contains("already have key package") =>
                    {
                        return Err(Error::KeyPackageAlreadyExists)
                    }
                    tonic::Code::InvalidArgument
                        if e.message().contains("Failed to add round2 dkg") =>
                    {
                        return Err(Error::FailedToAddRound2Package)
                    }
                    _ => return Err(Error::InternalGrpc),
                }
            }
        };
        Ok(())
    }

    pub(crate) fn coordinator_identifier(&self) -> frost::Identifier {
        // the 0th peer is always the coordinator
        peer_id_to_identifier(0)
    }

    pub(crate) async fn get_all_peers_handle(&self) -> Result<HashMap<PeerId, PeerData>, Error> {
        // get all frost peers connections
        let (peers_connections_sender, peers_connections_receiver) =
            tokio::sync::oneshot::channel::<HashMap<PeerId, PeerData>>();
        if let Err(e) = self
            .frost_handle
            .send_command(FrostCommand::GetAllConnectedPeers(peers_connections_sender))
        {
            error!(target: "consensus::authority::dkg::get_all_peers_handle", "Failed to send GetAllConnectedPeers frost command {}", e);
        }
        match peers_connections_receiver.await {
            Ok(connected_peers) => Ok(connected_peers),
            Err(e) => {
                error!("Failed to get frost peers connections {:?}", e);
                Err(Error::FailedToGetConnectedPeersHandles)
            }
        }
    }

    async fn gossip_to_coordinator(
        &self,
        dkg_payload: DkgPayload,
        response_type: DkgEventResponseType,
    ) -> Result<(), Error> {
        // TODO retry_exec
        // get all connected peers
        let connected_peers = self.get_all_peers_handle().await?;
        let coord_id = self.coordinator_identifier();
        let coordinator_peer = connected_peers.iter().find(|(_, peer_data)| {
            peer_data.frost_identifier.and_then(|id| Some(id == coord_id)).unwrap_or_default()
        });

        // Find the coord and send the message
        if let Some((_, coord_data)) = coordinator_peer {
            let resp = PeerMessageResponse::Dkg(DkgResponse {
                response_type: response_type.clone(),
                identifier: dkg_payload.identifier.clone(),
                data: dkg_payload.payload.clone(),
            });
            if let Some(sender) = coord_data.peer_commands_tx.as_ref() {
                sender.send(FrostPeerCommand::PeerMessage(resp)).map_err(Error::Send)?;
            }
        }
        Ok(())
    }

    async fn gossip_to_peers(
        &self,
        dkg_payload: DkgPayload,
        response_type: DkgEventResponseType,
    ) -> Result<(), Error> {
        // TODO retry_exec
        info!(target: "consensus::authority::dkg::gossip_to_peers", "gossiping message type {:?} to all peers", response_type);
        // get all connected peers
        let connected_peers = self.get_all_peers_handle().await?;
        let coord_id = self.coordinator_identifier();

        // Broadcast dkg round 1 package to all peers (excluding ourselves)
        for (_, peer_data) in connected_peers.iter() {
            if peer_data
                .frost_identifier
                .as_ref()
                .and_then(|id| Some(*id != coord_id))
                .unwrap_or_default()
            {
                let resp = PeerMessageResponse::Dkg(DkgResponse {
                    response_type: response_type.clone(),
                    identifier: dkg_payload.identifier.clone(),
                    data: dkg_payload.payload.clone(),
                });
                if let Some(sender) = peer_data.peer_commands_tx.as_ref() {
                    sender.send(FrostPeerCommand::PeerMessage(resp)).map_err(Error::Send)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn start_coordinator(&mut self) -> Result<(), Error> {
        // Should only be starting if we are the coordinator
        if self.personal_frost_identifier != self.coordinator_identifier() {
            warn!(target: "consensus::authority::dkg::start_coordinator", " Not the coordinator, ignoring start coordinator");
            return Ok(());
        }
        // Start by adding our own round 1 package to memory
        let our_round1 = self.get_round1_dkg_package().await?;
        info!(
            target: "consensus::authority::dkg::start_coordinator",
            "dkg1_package retrieved. Identifier Size:{:?}, Data Size: {:?}",
            our_round1.identifier.len(),
            our_round1.payload.len()
        );
        self.round1_packages.insert(our_round1.identifier, our_round1.payload);

        // Start by sending round 1 requests
        self.gossip_to_peers(
            DkgPayload {
                identifier: self.personal_frost_identifier.serialize().to_vec(),
                ..Default::default()
            },
            DkgEventResponseType::DkgRound1Request,
        )
        .await?;
        info!(target: "consensus::authority::dkg::start_coordinator", "round 1 sent to all peers");

        self.state = DKGState::Running;

        Ok(())
    }

    pub(crate) async fn process_round1_coordinator(
        &mut self,
        identifier: Vec<u8>,
        payload: Vec<u8>,
    ) -> Result<(), Error> {
        // If we are not the coordinator, we should not be processing this round 1 package response
        if self.personal_frost_identifier != self.coordinator_identifier() {
            warn!(
                target: "consensus::authority::dkg::process_round1_coordinator",
                "Not the coordinator, ignoring round 1 package"
            );
            return Ok(());
        }

        // return if the sending identifier is us
        if self.personal_frost_identifier == deserialize_frost_peer_id(identifier.clone())? {
            warn!(
                target: "consensus::authority::dkg::process_round1_coordinator",
                "Received our own round 1 package"
            );
            return Ok(());
        }
        // add the transmitted round 1 package data
        if let Err(e) = self.add_round1_dkg_package(identifier.clone(), payload.clone()).await {
            error!(
                target: "consensus::authority::dkg::process_round1_coordinator",
                "Error adding round 1 dkg package {:?}", e
            );
        }
        self.round1_packages.insert(identifier, payload);

        info!(
            target: "consensus::authority::dkg::process_round1_coordinator",
            "round 1 package added successfully"
        );
        // Check if we are ready to progress to round 2
        let dkg2_package = match self.get_round2_dkg_package().await {
            Ok(dkg2_package) => dkg2_package,
            Err(e) => {
                // its ok to error here if we don't have enough packages
                error!("Error getting round 2 dkg package {:?}", e);
                return Err(e);
            }
        };
        // Save our own round 2 package to memory
        self.round2_packages.insert(dkg2_package.identifier.clone(), dkg2_package.payload.clone());

        // We have suffecient round 1 messages to progress to round 2
        // Now we need to gossip each round 1 message to all peers
        // This includes our own round 1 package
        for (identifier, payload) in self.round1_packages.iter() {
            self.gossip_to_peers(
                DkgPayload { identifier: identifier.clone(), payload: payload.clone() },
                DkgEventResponseType::DkgRound1,
            )
            .await?;
        }

        Ok(())
    }

    pub(crate) async fn process_round1_request(&mut self) -> Result<(), Error> {
        let round1_package = self.get_round1_dkg_package().await?;
        // Gossip to coordinator
        self.gossip_to_coordinator(round1_package, DkgEventResponseType::DkgRound1).await?;
        Ok(())
    }

    pub(crate) async fn process_round1(
        &mut self,
        identifier: Vec<u8>,
        payload: Vec<u8>,
    ) -> Result<(), Error> {
        // Ensure we are not the coordinator
        if self.personal_frost_identifier == self.coordinator_identifier() {
            self.process_round1_coordinator(identifier, payload).await?;
            return Ok(());
        }
        info!(
            target: "consensus::authority::dkg::process_round1","identifiers {:?} {:?}",
            self.personal_frost_identifier,
            deserialize_frost_peer_id(identifier.clone())?
        );

        // return if the sending identifier is us
        if self.personal_frost_identifier == deserialize_frost_peer_id(identifier.clone())? {
            warn!(target: "consensus::authority::dkg::process_round1", "Received our own round 1 package");
            return Ok(());
        }

        // add the transmitted round 1 package data
        if let Err(e) = self.add_round1_dkg_package(identifier, payload).await {
            error!(target: "consensus::authority::dkg::process_round1", "Error adding round 1 dkg package {:?}", e);
        }
        info!(target: "consensus::authority::dkg::process_round1","package added successfully");
        // Check if we are ready to progress to round 2
        let dkg2_package = match self.get_round2_dkg_package().await {
            Ok(dkg2_package) => dkg2_package,
            Err(e) => {
                // its ok to error here if we don't have enough packages
                error!("Error getting round 2 dkg package {:?}", e);
                return Err(e);
            }
        };

        info!(target: "consensus::authority::dkg::process_round1", "ready to move to round 2");
        if let Err(e) =
            self.gossip_to_coordinator(dkg2_package, DkgEventResponseType::DkgRound2).await
        {
            error!(target: "consensus::authority::dkg::process_round1", "Error gossiping round 2 to peers {:?}", e);
            self.state = DKGState::DkgFailed;
            return Err(e);
        }

        Ok(())
    }

    pub(crate) async fn process_round2_coordinator(
        &mut self,
        identifier: Vec<u8>,
        payload: Vec<u8>,
    ) -> Result<(), Error> {
        // If we are not the coordinator, we should not be processing this round 1 package response
        if self.personal_frost_identifier != self.coordinator_identifier() {
            warn!(target: "consensus::authority::dkg::process_round2_coordinator", "Not the coordinator, ignoring round 1 package");
            return Ok(());
        }

        // return if the sending identifier is us
        if self.personal_frost_identifier == deserialize_frost_peer_id(identifier.clone())? {
            warn!(target: "consensus::authority::dkg::process_round2_coordinator", "Received our own round 1 package");
            return Ok(());
        }
        // Save in btc server
        self.add_round2_dkg_package(identifier.clone(), payload.clone()).await?;
        // Add to the round 2 packages
        self.round2_packages.insert(identifier, payload);

        // Once we get the pk we know we are done with round 2
        let agg_public_key = self.get_public_key().await?;
        info!(target: "consensus::authority::dkg::process_round2_coordinator", "Got pubkey_package: {:?}", agg_public_key.publickey);
        // gossip all the round 2 packages to all peers
        for (identifier, payload) in self.round2_packages.iter() {
            self.gossip_to_peers(
                DkgPayload { identifier: identifier.clone(), payload: payload.clone() },
                DkgEventResponseType::DkgRound2,
            )
            .await?;
        }

        self.process_round3().await?;

        Ok(())
    }

    pub(crate) async fn process_round2(
        &mut self,
        identifier: Vec<u8>,
        payload: Vec<u8>,
    ) -> Result<(), Error> {
        // ensure we are not the coordinator
        if self.personal_frost_identifier == self.coordinator_identifier() {
            self.process_round2_coordinator(identifier, payload).await?;
            return Ok(());
        }
        info!(target: "consensus::authority::dkg::process_round2",
            "our id: {:?},\n peers id:  {:?}",
            self.personal_frost_identifier,
            deserialize_frost_peer_id(identifier.clone())?
        );

        // return if the sending identifier is us
        if self.personal_frost_identifier == deserialize_frost_peer_id(identifier.clone())? {
            warn!(target: "consensus::authority::dkg::process_round2", "Received our own round 1 package");
            return Ok(());
        }

        // add the transmitted round 2 package data
        if let Err(e) = self.add_round2_dkg_package(identifier, payload).await {
            warn!(target: "consensus::authority::dkg::process_round2", "Error adding round 2 dkg package {:?}", e);
            // We dont want to fail the whole dkg process if we can't add another's round2
            return Ok(());
        }
        info!(target: "consensus::authority::dkg::process_round2", "packages added successfully");
        // By adding this round2 dkg package we could be ready to progress to round 3
        // Check first before gossiping and then gossip regardless
        // Lets try to progress to round 3 (getting the agg pk)
        let public_key_res = self.get_public_key().await;
        if public_key_res.is_ok() {
            info!(target: "consensus::authority::dkg::process_round2", "ready to move to round 3");
            self.process_round3().await?;
        }

        Ok(())
    }

    async fn process_round3(&mut self) -> Result<(), Error> {
        info!(target: "consensus::authority::dkg::process_round3", "Processing...");

        let public_key = match self.get_public_key().await {
            Ok(public_key) => public_key,
            Err(e) => {
                error!("Error getting public key package {:?}", e);
                return Err(e);
            }
        };
        info!(target: "consensus::authority::dkg::process_round3", "Got pubkey_package: {:?}", public_key.publickey);

        // decode the public key and assign it to the self variable
        self.public_key_package = match secp256k1::PublicKey::from_str(&public_key.publickey) {
            Ok(decoded_pubkey) => Some(decoded_pubkey),
            Err(e) => {
                error!(target: "consensus::authority::dkg::process_round3", "Error hex decoding public key {:?}", e);
                self.state = DKGState::DkgFailed;
                return Err(Error::PublicKeyParse(e));
            }
        };
        let mut storage = self.storage.write().await;
        storage.aggregate_public_key = self.public_key_package;
        drop(storage);
        info!(target: "consensus::authority::dkg::process_round3", "Round 3 finished successfully");
        Ok(())
    }
}
