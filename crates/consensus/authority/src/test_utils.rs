use btcserverlib::extended_client::{BtcServerExtendedApi, GrpcClientError};
use client::{
    DkgPayload, Empty, FinalizeSignerRequest, FinalizeSigningRequest, FinalizeSigningResponse,
    GetAllUtxosResponse, GetGatewayAddressRequest, GetGatewayAddressResponse, GetPublicKeyResponse,
    GetSessionIdsRequest, GetSessionIdsResponse, GetSigningStatusRequest, GetSigningStatusResponse,
    GetTrackedTxsResponse, MakeTxRequest, NotifyPeginsRequest, NotifyPegoutRequest,
    ResetAllUtxosRequest, ResetWalletStateRequest, SigningPackage, SigningPackageRequest,
    SyncTxIndexRequest, ToSignRequest, WalletStateResponse, GetPendingPegoutsResponse,
};
use alloy_rpc_types_engine::JwtSecret;
use frost_secp256k1_tr::{
    Identifier,
    keys::{
        dkg::{round1, round2, part1, part2, part3},
        KeyPackage, PublicKeyPackage,
    },
    round1::{SigningNonces, SigningCommitments, commit},
    round2::{SignatureShare, sign},
};
use rand::rngs::OsRng;
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};
use tokio::sync::RwLock;
use crate::dkg::DKGState;
use crate::signing::SigningState;
use anyhow::{Context, Error};

/// Helper struct to hold DKG and signing test data
#[derive(Clone)]
pub struct TestData {
    pub identifier: Identifier,
    pub round1_package: round1::Package,
    pub round1_secret: round1::SecretPackage,
    pub round2_packages: BTreeMap<Identifier, round2::Package>,
    pub round2_secret: round2::SecretPackage,
    pub key_package: KeyPackage,
    pub public_key_package: PublicKeyPackage,
    pub signing_round1_nonces: Option<SigningNonces>,
    pub signing_round1_commitments: Option<SigningCommitments>,
    pub signing_round2_share: Option<SignatureShare>,
}

/// Generate test vectors for DKG
pub fn generate_test_vectors(num_participants: u16, threshold: u16) -> Vec<TestData> {
    let mut test_data = Vec::with_capacity(num_participants as usize);
    let mut round1_packages = BTreeMap::new();
    let mut round1_secrets = Vec::with_capacity(num_participants as usize);
    let mut identifiers = Vec::with_capacity(num_participants as usize);

    // Generate round 1 packages for all participants
    for i in 0..num_participants {
        let identifier = Identifier::derive(&i.to_le_bytes()).expect("can derive identifier");
        identifiers.push(identifier);
        let (round1_secret, round1_package) = part1(
            identifier,
            num_participants,
            threshold,
            &mut OsRng,
        ).expect("can generate round1");

        round1_packages.insert(identifier, round1_package.clone());
        round1_secrets.push((identifier, round1_secret));
    }

    // Generate round 2 packages
    let mut all_round2_secrets = Vec::with_capacity(num_participants as usize);
    let mut all_round2_packages = Vec::with_capacity(num_participants as usize);

    for (identifier, round1_secret) in round1_secrets.iter() {
        let (round2_secret, round2_packages) = part2(
            round1_secret.clone(),
            &round1_packages,
        ).expect("can generate round2");

        all_round2_secrets.push((*identifier, round2_secret));
        all_round2_packages.push((*identifier, round2_packages));
    }

    // Generate key packages and public key packages
    for i in 0..num_participants as usize {
        let (key_package, public_key_package) = part3(
            &all_round2_secrets[i].1,
            &round1_packages,
            &all_round2_packages[i].1,
        ).expect("can generate key packages");

        test_data.push(TestData {
            identifier: identifiers[i],
            round1_package: round1_packages[&identifiers[i]].clone(),
            round1_secret: round1_secrets[i].1.clone(),
            round2_packages: all_round2_packages[i].1.clone(),
            round2_secret: all_round2_secrets[i].1.clone(),
            key_package,
            public_key_package,
            signing_round1_nonces: None,
            signing_round1_commitments: None,
            signing_round2_share: None,
        });
    }

    test_data
}

/// Generate signing test vectors for a participant
pub fn generate_signing_test_vectors(
    key_package: &KeyPackage,
) -> (SigningNonces, SigningCommitments) {
    commit(
        key_package.signing_share(),
        &mut OsRng,
    )
}

/// Generate round2 signing package for a participant
pub fn generate_signing_round2_package(
    key_package: &KeyPackage,
    round1_nonces: &SigningNonces,
    received_round1_packages: &BTreeMap<Identifier, SigningCommitments>,
    signing_session_id: [u8; 32],
) -> Result<SignatureShare, Error> {
    let signing_package = frost_secp256k1_tr::SigningPackage::new(
        received_round1_packages.clone(),
        &signing_session_id,
    );

    sign(
        &signing_package,
        round1_nonces,
        key_package,
    ).map_err(Error::from)
}

/// Mock BTC server client for testing
#[derive(Clone)]
pub struct MockBtcServerClient {
    pub peer_id: String,
    pub max_signers: u16,
    pub min_signers: u16,
    pub test_vectors: Vec<TestData>,
    pub added_round1_packages: BTreeMap<Vec<u8>, Vec<u8>>,
    pub added_round2_packages: BTreeMap<Vec<u8>, Vec<u8>>,
    pub added_round1_signing_packages: BTreeMap<Vec<u8>, Vec<u8>>,
    pub added_round2_signing_packages: BTreeMap<Vec<u8>, Vec<u8>>,
    pub public_key_package: Option<PublicKeyPackage>,
    pub state: BTreeMap<[u8; 32], SessionState>,
    pub dkg_state: DKGState,
    pub coordinator_map: BTreeMap<[u8; 32], u64>,
}

impl MockBtcServerClient {
    pub fn new(min_signers: u16, max_signers: u16, peer_id: String) -> Self {
        let test_vectors = generate_test_vectors(max_signers, min_signers);
        let peer_idx = peer_id.parse::<usize>().unwrap();
        let public_key_package = test_vectors[peer_idx].public_key_package.clone();

        Self {
            peer_id,
            max_signers,
            min_signers,
            test_vectors,
            added_round1_packages: BTreeMap::new(),
            added_round2_packages: BTreeMap::new(),
            added_round1_signing_packages: BTreeMap::new(),
            added_round2_signing_packages: BTreeMap::new(),
            public_key_package: Some(public_key_package),
            state: BTreeMap::new(),
            dkg_state: DKGState::Initial,
            coordinator_map: BTreeMap::new(),
        }
    }

    /// Helper method to generate round1 signing packages
    pub fn generate_round1_signing(&mut self) -> Result<(), Error> {
        let peer_idx = self.peer_id.parse::<usize>().unwrap_or(0);
        let test_data = &mut self.test_vectors[peer_idx];
        
        let (nonces, commitments) = generate_signing_test_vectors(
            &test_data.key_package,
        );

        test_data.signing_round1_nonces = Some(nonces);
        test_data.signing_round1_commitments = Some(commitments);
        Ok(())
    }

    /// Helper method to generate round2 signing packages
    pub fn generate_round2_signing(
        &mut self,
        session_id: [u8; 32],
        received_packages: BTreeMap<Identifier, SigningCommitments>,
    ) -> Result<(), Error> {
        let peer_idx = self.peer_id.parse::<usize>().context("Failed to parse peer_id as usize")?;
        let test_data = &mut self.test_vectors[peer_idx];
        
        let round1_nonces = test_data.signing_round1_nonces.as_ref()
        .ok_or_else(|| anyhow::anyhow!("Round1 nonces not generated yet"))?;

        let round2_share = generate_signing_round2_package(
            &test_data.key_package,
            round1_nonces,
            &received_packages,
            session_id,
        ).context("Failed to generate round2 package")?;

        test_data.signing_round2_share = Some(round2_share);
        Ok(())
    }

    fn deserialize_identifier(&self, bytes: &[u8]) -> Result<Identifier, GrpcClientError> {
        let id_array: [u8; 32] = bytes.try_into()
            .map_err(|_| GrpcClientError::Call(tonic::Status::internal("Invalid identifier length")))?;
        Identifier::deserialize(&id_array)
            .map_err(|e| GrpcClientError::Call(tonic::Status::internal(e.to_string())))
    }

    pub fn get_dkg_state(&self) -> DKGState {
        self.dkg_state
    }

    pub fn set_dkg_state(&mut self, state: DKGState) {
        self.dkg_state = state;
    }

    pub fn get_session_state(&self, session_id: [u8; 32]) -> Option<SessionState> {
        self.state.get(&session_id).cloned()
    }
    
    pub fn set_coordinator(&mut self, session_id: [u8; 32], coordinator: u64) {
        self.coordinator_map.insert(session_id, coordinator);
    }

    pub fn is_coordinator(&self, session_id: [u8; 32]) -> bool {
        self.coordinator_map.get(&session_id)
            .map(|coord| *coord == self.peer_id.parse().unwrap_or(0) as u64)
            .unwrap_or(false)
    }
}

// Add new types to track state
#[derive(Clone, Debug)]
pub struct SessionState {
    pub state: SigningState,
    pub round1_packages: usize,
    pub round2_packages: usize,
    pub coordinator: u64,
}

#[allow(unused_variables)]
impl BtcServerExtendedApi for MockBtcServerClient {
    fn update_jwt_secret(&mut self, _jwt_secret: JwtSecret) {}

    fn generate_jwt_token(&mut self) -> Option<String> {
        None
    }

    fn get_round1_dkg_package<'a>(&'a mut self, _: Empty) -> Pin<Box<dyn Future<Output = Result<DkgPayload, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move {
            let peer_idx = self.peer_id.parse::<usize>().unwrap_or(0);
            let test_data = &self.test_vectors[peer_idx];
            
            Ok(DkgPayload {
                identifier: test_data.identifier.serialize().to_vec(),
                payload: bincode::serialize(&test_data.round1_package)
                    .map_err(|e| GrpcClientError::Call(tonic::Status::internal(e.to_string())))?,
            })
        })
    }

    fn get_round2_dkg_package<'a>(&'a mut self, _: Empty) -> Pin<Box<dyn Future<Output = Result<DkgPayload, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move {
            let peer_idx = self.peer_id.parse::<usize>().unwrap_or(0);
            let test_data = &self.test_vectors[peer_idx];

            // Need packages from all participants before returning round2
            if self.added_round1_packages.len() < self.max_signers as usize {
                return Err(GrpcClientError::Call(tonic::Status::internal("Not enough round1 packages")));
            }

            Ok(DkgPayload {
                identifier: test_data.identifier.serialize().to_vec(),
                payload: bincode::serialize(&test_data.round2_packages)
                    .map_err(|e| GrpcClientError::Call(tonic::Status::internal(e.to_string())))?,
            })
        })
    }

    fn new_round1_dkg_package<'a>(
        &'a mut self,
        request: DkgPayload,
    ) -> Pin<Box<dyn Future<Output = Result<Empty, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move {
            self.added_round1_packages.insert(request.identifier.clone(), request.payload.clone());
            Ok(Empty {})
        })
    }

    fn new_round2_dkg_package<'a>(
        &'a mut self,
        request: DkgPayload,
    ) -> Pin<Box<dyn Future<Output = Result<Empty, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move {
            self.added_round2_packages.insert(request.identifier, request.payload);
            Ok(Empty {})
        })
    }

    fn get_public_key<'a>(&'a mut self, _: Empty) -> Pin<Box<dyn Future<Output = Result<GetPublicKeyResponse, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move {
            if self.added_round2_packages.len() < self.max_signers as usize {
                return Err(GrpcClientError::Call(tonic::Status::internal("Not enough round2 packages")));
            }

            let peer_idx = self.peer_id.parse::<usize>().unwrap_or(0);
            let test_data = &self.test_vectors[peer_idx];

            Ok(GetPublicKeyResponse {
                publickey: hex::encode(test_data.public_key_package.serialize().expect("serialize should not fail")),
            })
        })
    }

    fn get_round1_signing_package<'a>(
        &'a mut self,
        request: SigningPackageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<SigningPackage, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move {
            let peer_idx = self.peer_id.parse::<usize>().unwrap_or(0);
            
            let session_id: [u8; 32] = request.signing_session_id.try_into()
                .map_err(|_| GrpcClientError::Call(tonic::Status::internal("Invalid session id")))?;

            self.generate_round1_signing()
                .map_err(|e| GrpcClientError::Call(tonic::Status::internal(e.to_string())))?;

            let test_data = &self.test_vectors[peer_idx];
            let package = test_data.signing_round1_commitments.as_ref()
                .ok_or_else(|| GrpcClientError::Call(tonic::Status::internal("Round1 package not found")))?;

            Ok(SigningPackage {
                identifier: test_data.identifier.serialize().to_vec(),
                signing_session_id: session_id.to_vec(),
                psbt: bincode::serialize(package)
                    .map_err(|e| GrpcClientError::Call(tonic::Status::internal(e.to_string())))?,
            })
        })
    }

    fn get_round2_signing_package<'a>(
        &'a mut self,
        request: SigningPackageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<SigningPackage, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move {
            let peer_idx = self.peer_id.parse::<usize>().unwrap_or(0);
            let test_data = &self.test_vectors[peer_idx];
            
            let session_id: [u8; 32] = request.signing_session_id.try_into()
                .map_err(|_| GrpcClientError::Call(tonic::Status::internal("Invalid session id")))?;

            let package = test_data.signing_round2_share.as_ref()
                .ok_or_else(|| GrpcClientError::Call(tonic::Status::internal("Round2 package not generated yet")))?;

            Ok(SigningPackage {
                identifier: test_data.identifier.serialize().to_vec(),
                signing_session_id: session_id.to_vec(),
                psbt: bincode::serialize(package)
                    .map_err(|e| GrpcClientError::Call(tonic::Status::internal(e.to_string())))?,
            })
        })
    }

    fn new_round1_signing_package<'a>(
        &'a mut self,
        request: SigningPackage,
    ) -> Pin<Box<dyn Future<Output = Result<Empty, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move{
            self.added_round1_signing_packages.insert(request.identifier, request.psbt);
            Ok(Empty {})
        })
    }

    fn get_to_sign_package<'a>(
        &'a mut self,
        request: ToSignRequest,
    ) -> Pin<Box<dyn Future<Output = Result<SigningPackage, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move{
            let peer_idx = self.peer_id.parse::<usize>().unwrap_or(0);
            let test_data = &self.test_vectors[peer_idx];
            
            // Check if we have enough round1 signing packages
            if self.added_round1_signing_packages.len() <  self.min_signers as usize {
                return Err(GrpcClientError::Call(tonic::Status::internal("Not enough round1 signing packages")));
            }
            
            Ok(SigningPackage {
                identifier: test_data.identifier.serialize().to_vec(),
                signing_session_id: request.signing_session_id,
                psbt: Vec::new(),
            })
        })
    }

    fn new_round2_signing_package<'a>(
        &'a mut self,
        request: SigningPackage,
    ) -> Pin<Box<dyn Future<Output = Result<Empty, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move{
            self.added_round2_signing_packages.insert(request.identifier, request.psbt);
            Ok(Empty {})
        })
    }

    fn finalize_signing<'a>(
        &'a mut self,
        request: FinalizeSigningRequest,
    ) -> Pin<Box<dyn Future<Output = Result<FinalizeSigningResponse, GrpcClientError>> + Send + 'a>>{
        Box::pin(async move{
            if self.added_round2_signing_packages.len() < self.min_signers as usize {
                return Err(GrpcClientError::Call(tonic::Status::internal("Not enough round2 signing packages")));
            }

            let peer_idx = self.peer_id.parse::<usize>().unwrap_or(0);
            let test_data = &self.test_vectors[peer_idx];

            // Get round1 commitments and original PSBT
            let psbt = self.added_round1_signing_packages.values().next()
                .ok_or_else(|| GrpcClientError::Call(tonic::Status::internal("No round1 packages found")))?.clone();
            Ok(FinalizeSigningResponse { psbt })
        })
    }

    fn abort_signing<'a>(&'a mut self, _: Empty) -> Pin<Box<dyn Future<Output = Result<Empty, GrpcClientError>> + Send + 'a>> {
        // Clear all signing related state
        Box::pin(async move{
            self.added_round1_signing_packages.clear();
            self.added_round2_signing_packages.clear();

            // Reset signing state in test vectors
            for test_data in &mut self.test_vectors {
                test_data.signing_round1_nonces = None;
                test_data.signing_round1_commitments = None;
                test_data.signing_round2_share = None;
            }
            
            Ok(Empty {})
        })
    }

    fn get_signing_status<'a>(
        &'a mut self,
        request: GetSigningStatusRequest,
    ) -> Pin<Box<dyn Future<Output = Result<GetSigningStatusResponse, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move{
            let session_id: [u8; 32] = request.signing_session_id.try_into()
            .map_err(|_| GrpcClientError::Call(tonic::Status::internal("Invalid session id")))?;

            let session = self.state.get(&session_id)
                .ok_or_else(|| GrpcClientError::Call(tonic::Status::not_found("Session not found")))?;

            // Return actual session state
            Ok(GetSigningStatusResponse {
                status: match session.state {
                    SigningState::Initial => 0,
                    SigningState::Round1 => 1, 
                    SigningState::Round2 => 2,
                    SigningState::Finalized => 3,
                    SigningState::Failed => 4,
                }
            })
        })
    }

    // Implement other required methods with default/empty implementations
    fn notify_pegins<'a>(&'a mut self, _: NotifyPeginsRequest) -> Pin<Box<dyn Future<Output = Result<Empty, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move {
            unimplemented!("Not required for core DKG/signing tests")
        })
    }

    fn notify_pegout<'a>(&'a mut self, _: NotifyPegoutRequest) -> Pin<Box<dyn Future<Output = Result<Empty, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move {
            unimplemented!("Not required for core DKG/signing tests")
        })
    }

    fn get_gateway_address<'a>(
        &'a mut self,
        _: GetGatewayAddressRequest,
    ) -> Pin<Box<dyn Future<Output = Result<GetGatewayAddressResponse, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move {
            unimplemented!("Not required for core DKG/signing tests")
        })
    }

    fn get_round1_dkg_packages<'a>(&'a mut self, _: Empty) -> Pin<Box<dyn Future<Output = Result<DkgPayload, GrpcClientError>> + Send + 'a>> {
        Box::pin(async move {
            unimplemented!("Not required for core DKG/signing tests")
        })
    }

    fn get_psbt<'a>(&'a mut self, request: MakeTxRequest) -> Pin<Box<dyn Future<Output = Result<SigningPackage, GrpcClientError>> + Send + 'a>> {
        unimplemented!("Not required for core DKG/signing tests")
    }

    fn signer_finalize<'a>(
        &'a mut self,
        request: FinalizeSignerRequest,
    ) -> Pin<Box<dyn Future<Output = Result<FinalizeSigningResponse, GrpcClientError>> + Send + 'a>> {
        unimplemented!("Not required for DKG/signing tests")
    }

    fn get_wallet_state<'a>(&'a mut self, _: Empty) -> Pin<Box<dyn Future<Output = Result<WalletStateResponse, GrpcClientError>> + Send + 'a>> {
        unimplemented!("Not required for DKG/signing tests")
    }

    fn get_session_ids<'a>(
        &'a mut self,
        request: GetSessionIdsRequest,
    ) -> Pin<Box<dyn Future<Output = Result<GetSessionIdsResponse, GrpcClientError>> + Send + 'a>> {
        unimplemented!("Not required for core signing functionality") 
    }

    fn health_check<'a>(&'a mut self, _: Empty) -> Pin<Box<dyn Future<Output = Result<Empty, GrpcClientError>> + Send + 'a>> {
        unimplemented!("Not required for DKG/signing tests")
    }

    fn tx_index_new_checkpoint<'a>(
        &'a mut self,
        request: SyncTxIndexRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Empty, GrpcClientError>> + Send + 'a>> {
        unimplemented!("Not required for DKG/signing tests")
    }

    fn reset_all_utxos<'a>(&'a mut self, _: ResetAllUtxosRequest) -> Pin<Box<dyn Future<Output = Result<Empty, GrpcClientError>> + Send + 'a>> {
        unimplemented!("Not required for DKG/signing tests")
    }

    fn get_all_utxos<'a>(&'a mut self, _: Empty) -> Pin<Box<dyn Future<Output = Result<GetAllUtxosResponse, GrpcClientError>> + Send + 'a>> {
        unimplemented!("Not required for DKG/signing tests")
    }

    fn get_tracked_txs<'a>(&'a mut self, _: Empty) -> Pin<Box<dyn Future< Output = Result<GetTrackedTxsResponse, GrpcClientError>> + Send + 'a>> {
        unimplemented!("Not required for DKG/signing tests")
    }

    fn get_pending_pegouts<'a>(&'a mut self, _: Empty) -> Pin<Box<dyn Future<Output = Result<GetPendingPegoutsResponse, GrpcClientError>> + Send + 'a>> {
        unimplemented!("Not required for DKG/signing tests")
    }

    fn reset_wallet_state<'a>(&'a mut self, _: ResetWalletStateRequest) -> Pin<Box<dyn Future<Output = Result<Empty, GrpcClientError>> + Send + 'a>> {
        unimplemented!("Not required for DKG/signing tests")
    }
}