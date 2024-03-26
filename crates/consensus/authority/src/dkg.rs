use crate::{
    extended_client::BtcServerExtendedClient,
    utils::{deserialize_frost_peer_id, FrostParseError},
    Storage,
};
use client::{DkgPayload, Empty, GetPublicKeyResponse};
use frost_secp256k1_tr as frost;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_network::frost::{
    manager::{peer_id_to_identifier, FrostCommand, FrostConfig, FrostHandle},
    DkgEventResponseType, DkgResponse, FrostPeerCommand, PeerMessageResponse,
};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use std::{
    collections::{BTreeMap, HashMap},
    str::FromStr,
};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info, warn};

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
    #[error("Error when parsing round 1 packages")]
    Round1PackageParse(serde_json::Error),
    #[error("Error when serializing round 1 packages")]
    Round1PackageSerialize,
    #[error("Error when parsing round 2 packages")]
    Round2PackageParse(serde_json::Error),
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
    Round1Start,
    /// Round 1 of dkg is running
    Round1Waiting,
    /// Round 2 of dkg has been started
    Round2Start,
    /// Round 2 of dkg is running
    Round2Waiting,
    /// Round 3 of dkg has been started
    Round3,
    /// The dkg state machine has failed
    DkgFailed,
}

impl DKGState {
    /// Returns true if the DKG state machine is in a running state
    pub(crate) fn is_running(&self) -> bool {
        match self {
            DKGState::Initial => false,
            _ => true,
        }
    }
    /// Returns true if we are in round 1 of dkg
    pub(crate) fn is_round1(&self) -> bool {
        match self {
            DKGState::Initial | DKGState::Round1Start | DKGState::Round1Waiting => true,
            _ => false,
        }
    }
    /// Returns true if we are in round 2 of dkg
    pub(crate) fn is_round2(&self) -> bool {
        match self {
            DKGState::Round2Start | DKGState::Round2Waiting => true,
            _ => false,
        }
    }
}

/// A state machine for transitioning between different DKG states
#[derive(Debug, Clone)]
pub(crate) struct DKGStateMachine<Client> {
    btc_client: BtcServerExtendedClient,
    storage: Storage<Client>,
    frost_handle: FrostHandle,
    state: DKGState,
    personal_frost_identifier: frost::Identifier,
    public_key_package: Option<secp256k1::PublicKey>,
    frost_config: FrostConfig,
}

impl<Client> DKGStateMachine<Client>
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
            state: DKGState::Initial,
            personal_frost_identifier,
            public_key_package: None,
            frost_config,
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
        }
    }

    /// Returns the public key package
    #[allow(dead_code)]
    pub(crate) fn get_public_key_package(&self) -> Option<secp256k1::PublicKey> {
        self.public_key_package.clone()
    }

    /// Returns the state machine state
    pub(crate) fn get_dkg_state(&self) -> DKGState {
        self.state
    }
}

impl<Client> DKGStateMachine<Client>
where
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
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
        let round3_payload = self.btc_client.get_public_key(Empty {}).await; // TODO: fix me
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

    pub(crate) async fn gossip_round2_to_peers(
        &mut self,
        dkg_payload: DkgPayload,
    ) -> Result<(), Error> {
        // get all connected peers
        let connected_peers = self.get_all_peers_handle().await?;

        // Broadcast dkg round 1 package to all peers (excluding ourselves)
        connected_peers.iter().for_each(|(frost_id, sender)| {
            if *frost_id != self.personal_frost_identifier {
                let resp = PeerMessageResponse::Dkg(DkgResponse {
                    response_type: DkgEventResponseType::DkgRound2,
                    identifier: dkg_payload.identifier.clone(),
                    data: dkg_payload.payload.clone(),
                });
                let _ = sender.send(FrostPeerCommand::PeerMessage(resp)); // TODO: map to error ?
            }
        });
        Ok(())
    }

    pub(crate) async fn gossip_round1_to_peers(&mut self) -> Result<(), Error> {
        // get round 1 package from db, if missing, create it
        let dkg1_package = self.get_round1_dkg_package().await?;
        println!("dkg1_package: {:?}", dkg1_package);

        // get all connected peers
        let connected_peers = self.get_all_peers_handle().await?;

        // Broadcast dkg round 1 package to all peers (excluding ourselves)
        connected_peers.iter().for_each(|(frost_id, sender)| {
            if *frost_id != self.personal_frost_identifier {
                let resp = PeerMessageResponse::Dkg(DkgResponse {
                    response_type: DkgEventResponseType::DkgRound1,
                    identifier: dkg1_package.identifier.clone(),
                    data: dkg1_package.payload.clone(),
                });
                let _ = sender.send(FrostPeerCommand::PeerMessage(resp)); // TODO: map to error ?
            }
        });
        Ok(())
    }

    pub(crate) async fn start(&mut self) -> Result<(), Error> {
        self.state = DKGState::Round1Start;
        info!(">>>>>>>>>>> [START] sending round 1 package to all peers");

        // get round 1 package from db and send it to all peers
        if let Err(e) = self.gossip_round1_to_peers().await {
            error!("Error gossiping round 1 to peers {:?}", e);
            self.state = DKGState::DkgFailed;
            return Err(e);
        }

        info!(">>>>>>>>>>> [START] round 1 sent to all peers");

        // Once the round 1 package is sent we are waiting
        self.state = DKGState::Round1Waiting;

        Ok(())
    }

    pub(crate) async fn process_round1(
        &mut self,
        identifier: Vec<u8>,
        payload: Vec<u8>,
    ) -> Result<(), Error> {
        // return if we are not in round 1
        if !self.state.is_round1() {
            return Ok(());
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
        if let Err(e) = self.add_round1_dkg_package(identifier, payload).await {
            error!("Error adding round 1 dkg package {:?}", e);
            self.state = DKGState::DkgFailed;
            return Err(e);
        }
        info!(">>>>>>>>>>> [PROCESS_ROUND1] package added successfully");
        info!(">>>>>>>>>>> [PROCESS_ROUND1] further gossiping round 1 packages to all peers...");
        // get round 1 package from db and send it to all peers
        if let Err(e) = self.gossip_round1_to_peers().await {
            error!("Error gossiping round 1 to peers {:?}", e);
            self.state = DKGState::DkgFailed;
            return Err(e);
        }

        // Check if we are ready to progress to round 2
        let dkg2_package = match self.get_round2_dkg_package().await {
            Ok(dkg2_package) => dkg2_package,
            Err(e) => {
                error!("Error getting round 2 dkg package {:?}", e);
                self.state = DKGState::DkgFailed;
                return Err(e);
            }
        };
        let round2_group_packages: BTreeMap<frost::Identifier, frost::keys::dkg::round2::Package> =
            match serde_json::from_slice(&dkg2_package.payload).map_err(Error::Round2PackageParse) {
                Ok(packages) => packages,
                Err(e) => {
                    error!("Error trying to parse round 1 dkg package {:?}", e);
                    self.state = DKGState::DkgFailed;
                    return Err(e);
                }
            };

        if round2_group_packages.len() >= (self.frost_config.max_signers - 1) as usize {
            info!(">>>>>>>>>>> [PROCESS_ROUND1] ready to move to round 2");
            if let Err(e) = self.gossip_round2_to_peers(dkg2_package).await {
                error!("Error gossiping round 2 to peers {:?}", e);
                self.state = DKGState::DkgFailed;
                return Err(e);
            }

            self.state = DKGState::Round2Start;
        } else {
            self.state = DKGState::Round1Waiting;
        }

        Ok(())
    }

    pub(crate) async fn process_round2(
        &mut self,
        identifier: Vec<u8>,
        payload: Vec<u8>,
    ) -> Result<(), Error> {
        // return if we are not in round 2
        if !self.state.is_round2() {
            return Ok(());
        }
        info!(
            ">>>>>>>>>>> [PROCESS_ROUND2] identifiers {:?} {:?}",
            self.personal_frost_identifier,
            deserialize_frost_peer_id(identifier.clone())?
        );

        // return if the sending identifier is us
        if self.personal_frost_identifier == deserialize_frost_peer_id(identifier.clone())? {
            return Ok(());
        }

        // add the transmitted round 2 package data
        if let Err(e) = self.add_round2_dkg_package(identifier, payload).await {
            warn!("Error adding round 2 dkg package {:?}", e);
            // We dont want to fail the whole dkg process if we can't add another's round2
            return Ok(())
        }
        info!(">>>>>>>>>>> [PROCESS_ROUND2] packages added successfully");
        // By adding this round2 dkg package we could be ready to progress to round 3
        // Check first before gossiping and then gossip regardless
        // Lets try to progress to round 3 (getting the agg pk)
        let public_key_res = self.get_public_key().await;
        if public_key_res.is_ok() {
            info!(">>>>>>>>>>> [PROCESS_ROUND2] ready to move to round 3");
            self.state = DKGState::Round3;
            self.process_round3().await?;
        } else {
            self.state = DKGState::Round2Waiting;
        }

        let round2_payload = match self.get_round2_dkg_package().await {
            Ok(round2_payload) => round2_payload,
            Err(e) => {
                error!("Error getting round2 dkg package {:?}", e);
                self.state = DKGState::DkgFailed;
                return Err(e);
            }
        };

        info!(">>>>>>>>>>> [PROCESS_ROUND2] further gossiping round 2 packages to all peers...");
        // get round 2 package from db and send it to all peers
        if let Err(e) = self.gossip_round2_to_peers(round2_payload).await {
            error!("Error gossiping round 2 to peers {:?}", e);
            self.state = DKGState::DkgFailed;
            return Err(e);
        }

        Ok(())
    }

    async fn process_round3(&mut self) -> Result<(), Error> {
        if self.state != DKGState::Round3 {
            return Ok(());
        }

        info!(">>>>>>>>>>> [PROCESS_ROUND3] Processing...");

        let public_key = match self.get_public_key().await {
            Ok(public_key) => public_key,
            Err(e) => {
                error!("Error getting public key package {:?}", e);
                self.state = DKGState::DkgFailed;
                return Err(e);
            }
        };
        info!(">>>>>>>>>>> [PROCESS_ROUND3] Got pubkey_package: {:?}", public_key.publickey);

        // decode the public key and assign it to the self variable
        self.public_key_package = match secp256k1::PublicKey::from_str(&public_key.publickey) {
            Ok(decoded_pubkey) => Some(decoded_pubkey),
            Err(e) => {
                error!("Error hex decoding public key {:?}", e);
                self.state = DKGState::DkgFailed;
                return Err(Error::PublicKeyParse(e));
            }
        };
        let mut storage = self.storage.write().await;
        storage.aggregate_public_key = self.public_key_package;
        drop(storage);
        info!(">>>>>>>>>>> [PROCESS_ROUND3] Round 3 finished successfully");
        Ok(())
    }
}
