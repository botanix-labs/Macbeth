use client::{BtcServerClient, DkgPayload, GetPublicKeyResponse};
use frost_secp256k1_tr as frost;
use reth_network::frost::{
    manager::{FrostCommand, FrostHandle},
    EventResponseType, FrostPeerCommand, Response,
};
use std::{collections::HashMap, str::FromStr};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Requested Key Package already exists")]
    KeyPackageAlreadyExists,
    #[error("Round 1 package is missing")]
    MissingRound1Package,
    #[error("Round 2 package is missing")]
    MissingRound2Package,
    #[error("Failed to get Round 1 package")]
    FailedToGetRound1Package,
    #[error("Failed to get Round 2 package")]
    FailedToGetRound2Package,
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
            DKGState::Round1Start | DKGState::Round1Waiting => true,
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
pub(crate) struct DKGStateMachine {
    btc_client: BtcServerClient<tonic::transport::Channel>,
    frost_handle: FrostHandle,
    state: DKGState,
    personal_identifier: frost::Identifier,
    public_key_package: Option<secp256k1::PublicKey>,
    min_signers: u16,
    max_signers: u16,
}

impl DKGStateMachine {
    /// Constructs a new state machine with the given params
    pub(crate) fn new(
        btc_client: BtcServerClient<tonic::transport::Channel>,
        frost_handle: FrostHandle,
        personal_identifier: frost::Identifier,
        min_signers: u16,
        max_signers: u16,
    ) -> Self {
        Self {
            btc_client,
            frost_handle,
            state: DKGState::Initial,
            personal_identifier,
            public_key_package: None,
            min_signers,
            max_signers,
        }
    }

    /// Resets the state machine to its initial state
    #[allow(dead_code)]
    pub(crate) fn reset(self) -> Self {
        Self {
            btc_client: self.btc_client,
            frost_handle: self.frost_handle,
            state: DKGState::Initial,
            personal_identifier: self.personal_identifier,
            public_key_package: None,
            min_signers: self.min_signers,
            max_signers: self.max_signers,
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

impl DKGStateMachine {
    async fn get_round1_dkg_package(&mut self) -> Result<DkgPayload, Error> {
        let round1_payload =
            self.btc_client.get_round1_dkg_package(tonic::Request::new(client::Empty {})).await;

        let round1_payload = match round1_payload {
            Ok(round1_payload) => round1_payload.into_inner(),
            Err(e) => match e.code() {
                tonic::Code::AlreadyExists if e.message().contains("already have key package") => {
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
                tonic::Code::Internal if e.message().contains("Failed to generate round 1 dkg") => {
                    return Err(Error::FailedToGenerateRound1Package)
                }
                _ => return Err(Error::InternalGrpc),
            },
        };
        Ok(round1_payload)
    }

    async fn get_round2_dkg_package(&mut self) -> Result<DkgPayload, Error> {
        let round2_payload =
            self.btc_client.get_round2_dkg_package(tonic::Request::new(client::Empty {})).await;

        let round2_payload = match round2_payload {
            Ok(round2_payload) => round2_payload.into_inner(),
            Err(e) => match e.code() {
                tonic::Code::AlreadyExists if e.message().contains("already have key package") => {
                    return Err(Error::KeyPackageAlreadyExists)
                }
                tonic::Code::Internal if e.message().contains("Missing round1 dkg package") => {
                    return Err(Error::MissingRound1Package)
                }
                tonic::Code::Internal
                    if e.message().contains("Failed to get round2 dkg packages") =>
                {
                    return Err(Error::FailedToGetRound2Package)
                }
                tonic::Code::InvalidArgument
                    if e.message().contains("Failed to generate round 2 dkg") =>
                {
                    return Err(Error::FailedToGenerateRound2Package)
                }
                _ => return Err(Error::InternalGrpc),
            },
        };
        Ok(round2_payload)
    }

    async fn get_public_key(&mut self) -> Result<GetPublicKeyResponse, Error> {
        let round3_payload =
            self.btc_client.get_public_key(tonic::Request::new(client::Empty {})).await;
        let round3_payload = match round3_payload {
            Ok(round3_payload) => round3_payload.into_inner(),
            Err(e) => match e.code() {
                tonic::Code::Internal
                    if e.message().contains("Failed to get public key package") =>
                {
                    return Err(Error::FailedToGetPubKeyPackage)
                }
                tonic::Code::Internal
                    if e.message().contains("Failed to get round1 dkg packages") =>
                {
                    return Err(Error::FailedToGetRound1Package)
                }
                tonic::Code::Internal
                    if e.message().contains("Failed to get round2 dkg packages") =>
                {
                    return Err(Error::FailedToGetRound2Package)
                }
                tonic::Code::Internal
                    if e.message().contains("Failed to generate public key package") =>
                {
                    return Err(Error::FailedToGeneratePubKeyPackage)
                }
                tonic::Code::Internal if e.message().contains("Failed to store key package") => {
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
            },
        };
        Ok(round3_payload)
    }

    async fn add_round1_dkg_package(
        &mut self,
        identifier: Vec<u8>,
        payload: Vec<u8>,
    ) -> Result<(), Error> {
        let req = client::DkgPayload { identifier, payload };
        match self.btc_client.new_round1_dkg_package(tonic::Request::new(req)).await {
            Ok(round2_payload) => round2_payload.into_inner(),
            Err(e) => match e.code() {
                tonic::Code::AlreadyExists if e.message().contains("already have key package") => {
                    return Err(Error::KeyPackageAlreadyExists)
                }
                tonic::Code::InvalidArgument
                    if e.message().contains("Failed to add round1 dkg") =>
                {
                    return Err(Error::FailedToAddRound1Package)
                }
                _ => return Err(Error::InternalGrpc),
            },
        };
        Ok(())
    }

    async fn add_round2_dkg_package(
        &mut self,
        identifier: Vec<u8>,
        payload: Vec<u8>,
    ) -> Result<(), Error> {
        let req = client::DkgPayload { identifier, payload };
        match self.btc_client.new_round2_dkg_package(tonic::Request::new(req)).await {
            Ok(round2_payload) => round2_payload.into_inner(),
            Err(e) => match e.code() {
                tonic::Code::AlreadyExists if e.message().contains("already have key package") => {
                    return Err(Error::KeyPackageAlreadyExists)
                }
                tonic::Code::InvalidArgument
                    if e.message().contains("Failed to add round2 dkg") =>
                {
                    return Err(Error::FailedToAddRound2Package)
                }
                _ => return Err(Error::InternalGrpc),
            },
        };
        Ok(())
    }

    pub(crate) async fn start(
        &mut self,
        connected_peers: HashMap<frost::Identifier, UnboundedSender<FrostPeerCommand>>,
    ) -> Result<(), Error> {
        self.state = DKGState::Round1Start;
        info!(
            ">>>>>>>>>>> Starting DKG, sending round 1 package to all peers. Total peers = {:?}",
            connected_peers.len()
        );

        // get round 1 package from db, if missing, create it
        let dkg1_package = match self.get_round1_dkg_package().await {
            Ok(dkg1_package) => dkg1_package,
            Err(e) => {
                // TODO: do what ?
                error!("Error getting round 1 dkg package {:?}", e);
                return Err(e);
            }
        };
        info!(">>>>>>>>>>> Round1 dkg package = {:?}", dkg1_package);

        // Broadcast dkg round 1 package to all peers (excluding ourselves)
        connected_peers.iter().for_each(|(frost_id, sender)| {
            if *frost_id != self.personal_identifier {
                let resp = Response {
                    response_type: EventResponseType::DkgRound1,
                    identifier: dkg1_package.identifier.clone(),
                    data: dkg1_package.payload.clone(),
                };
                info!(">>>>>>>>>>> Sending Round1Dkg ...");
                let _ = sender.send(FrostPeerCommand::PeerMessage(resp)); // TODO: handle error
                info!(">>>>>>>>>>> Command::Round1Dkg send!");
            }
        });
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

        // add the transmitted round 1 package data
        info!(">>>>>>>>>>> Adding the round 1 received package");
        self.add_round1_dkg_package(identifier, payload).await?;

        info!(">>>>>>>>>>> Checking for transition to round 2");
        let round1_payload = match self.get_round1_dkg_package().await {
            Ok(round1_payload) => round1_payload,
            Err(e) => {
                error!("Error getting round 1 dkg package {:?}", e);
                return Err(e);
            } // TODO: do what ?
        };
        let round1_group_packages: HashMap<frost::Identifier, frost::keys::dkg::round2::Package> =
            match serde_json::from_slice(&round1_payload.payload).map_err(Error::Round1PackageParse)
            {
                Ok(packages) => packages,
                Err(e) => {
                    return Err(e);
                } // TODO: do what ?
            };

        // Check if we are ready to progress to round 2
        if round1_group_packages.len() >= (self.max_signers - 1) as usize {
            // generate round 2 package using the btc server
            let dkg2_package = match self.get_round2_dkg_package().await {
                Ok(dkg2_package) => dkg2_package,
                Err(e) => {
                    error!("Error getting round 2 dkg package {:?}", e);
                    return Err(e);
                } // TODO: do what ?
            };

            // get all frost peers connections
            let (peers_connections_sender, peers_connections_receiver) =
                tokio::sync::oneshot::channel::<
                    HashMap<frost::Identifier, UnboundedSender<FrostPeerCommand>>,
                >();
            self.frost_handle
                .send_command(FrostCommand::GetAllConnectedFrostPeers(peers_connections_sender));
            match peers_connections_receiver.await {
                Ok(connected_peers) => {
                    info!(">>>>>>>>>>> Starting the DKG state machine...");
                    // Broadcast dkg round 2 packages to all peers (excluding ourselves)
                    connected_peers.iter().for_each(|(frost_id, sender)| {
                        if *frost_id != self.personal_identifier {
                            let resp = Response {
                                response_type: EventResponseType::DkgRound2,
                                identifier: dkg2_package.identifier.clone(),
                                data: dkg2_package.payload.clone(),
                            };
                            let _ = sender.send(FrostPeerCommand::PeerMessage(resp)); // TODO: handle error
                            println!(">>>>>>>>>>> Command::Round2Dkg send!");
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to get frost peers connections {:?}", e);
                }
            }
            info!(">>>>>>>>>>> Progressing to round 2");
            self.state = DKGState::Round2Start;
        } else {
            info!(">>>>>>>>>>> Round 1 Waiting...");
            self.state = DKGState::Round1Waiting;
        }

        Ok(())
    }

    pub(crate) async fn process_round2(
        &mut self,
        identifier: Vec<u8>,
        payload: Vec<u8>,
    ) -> Result<(), Error> {
        if !self.state.is_round2() {
            return Ok(());
        }

        // add the transmitted round 2 package data
        info!(">>>>>>>>>>> Adding the round 2 received package");
        self.add_round2_dkg_package(identifier, payload).await?;

        info!(">>>>>>>>>>> Checking for transition to round 3...");
        let round2_payload = match self.get_round2_dkg_package().await {
            Ok(round2_payload) => round2_payload,
            Err(e) => {
                return Err(e);
            } // TODO: do what ?
        };
        let round2_group_packages: HashMap<frost::Identifier, frost::keys::dkg::round2::Package> =
            match serde_json::from_slice(&round2_payload.payload).map_err(Error::Round2PackageParse)
            {
                Ok(packages) => packages,
                Err(e) => {
                    error!("Error parsing round2 group packages {:?}", e);
                    return Err(e);
                } // TODO: do what ?
            };

        // Check if we are ready to progress to round 3
        if round2_group_packages.len() >= (self.max_signers - 1) as usize {
            self.state = DKGState::Round3;
            self.process_round3().await?;
        } else {
            info!(">>>>>>>>>>> Round 2 Waiting...");
            self.state = DKGState::Round2Waiting;
        }

        Ok(())
    }

    async fn process_round3(&mut self) -> Result<(), Error> {
        if self.state != DKGState::Round3 {
            return Ok(());
        }

        info!(">>>>>>>>>>> Processing round 3 ...");

        let public_key = match self.get_public_key().await {
            Ok(public_key) => public_key,
            Err(e) => {
                error!("Error getting public key package {:?}", e);
                return Err(e);
            } // TODO: do error handling ?
        };
        info!(">>>>>>>>>>> Got pubkey_package: {:?}", public_key.publickey);

        // decode the public key and assign it to the self variable
        self.public_key_package = match secp256k1::PublicKey::from_str(&public_key.publickey) {
            Ok(decoded_pubkey) => Some(decoded_pubkey),
            Err(e) => {
                error!("Error hex decoding public key {:?}", e);
                return Err(Error::PublicKeyParse(e));
            }
        };

        info!(">>>>>>>>>>> Round 3 finished successfully");
        Ok(())
    }
}
