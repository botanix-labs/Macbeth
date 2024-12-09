use std::sync::{Arc, Mutex};
use btcserverlib::extended_client::{BtcServerExtendedClient, GrpcClientError};
use alloy_rpc_types_engine::JwtSecret;
use client::{
    DkgPayload, Empty, FinalizeSignerRequest, FinalizeSigningRequest, FinalizeSigningResponse,
    GetAllUtxosResponse, GetGatewayAddressRequest, GetGatewayAddressResponse, GetPublicKeyResponse,
    GetSessionIdsRequest, GetSessionIdsResponse, GetSigningStatusRequest, GetSigningStatusResponse,
    MakeTxRequest, NotifyPeginsRequest, NotifyPegoutRequest, ResetAllUtxosRequest, SigningPackage,
    SigningPackageRequest, SyncTxIndexRequest, ToSignRequest, WalletStateResponse,
};
use reth_network::frost::manager::{FrostCommand, ToFrostManager, PeerData};
use tokio::sync::mpsc::{self, error::SendError};
use std::collections::HashMap;
use reth_network_peers::PeerId;
use crate::Storage;
use reth_chainspec::ChainSpec;
use reth_node_ethereum::EthEvmConfig;
use std::net::SocketAddr;

#[derive(Debug, Clone, Default)]
pub(crate) struct MockBtcServerClient {
    pub round1_dkg_package: Arc<Mutex<Option<DkgPayload>>>,
    pub round2_dkg_package: Arc<Mutex<Option<DkgPayload>>>,
    pub public_key: Arc<Mutex<Option<GetPublicKeyResponse>>>,
    pub jwt_secret: Arc<Mutex<Option<JwtSecret>>>,
}

impl MockBtcServerClient {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_round1_package(mut self, package: DkgPayload) -> Self {
        *self.round1_dkg_package.lock().unwrap() = Some(package);
        self
    }

    pub(crate) fn with_round2_package(mut self, package: DkgPayload) -> Self {
        *self.round2_dkg_package.lock().unwrap() = Some(package);
        self
    }

    pub fn with_public_key(mut self, public_key: GetPublicKeyResponse) -> Self {
        *self.public_key.lock().unwrap() = Some(public_key);
        self
    }
}

#[allow(unused_variables)]
impl BtcServerExtendedClient for MockBtcServerClient {
    fn update_jwt_secret(&mut self, jwt_secret: JwtSecret) {
        *self.jwt_secret.lock().unwrap() = Some(jwt_secret);
    }

    fn generate_jwt_token(&mut self) -> Option<String> {
        None
    }

    async fn notify_pegins(&mut self, request: NotifyPeginsRequest) -> Result<Empty, GrpcClientError> {
        Ok(Empty {})
    }

    async fn notify_pegout(&mut self, request: NotifyPegoutRequest) -> Result<Empty, GrpcClientError> {
        Ok(Empty {})
    }

    async fn get_gateway_address(
        &mut self,
        request: GetGatewayAddressRequest,
    ) -> Result<GetGatewayAddressResponse, GrpcClientError> {
        Ok(GetGatewayAddressResponse::default())
    }

    async fn get_public_key(&mut self, request: Empty) -> Result<GetPublicKeyResponse, GrpcClientError> {
        self.public_key
            .lock()
            .unwrap()
            .clone()
            .ok_or(GrpcClientError::Call(tonic::Status::not_found("Public key not set")))
    }

    async fn get_round1_dkg_package(&mut self, request: Empty) -> Result<DkgPayload, GrpcClientError> {
        self.round1_dkg_package
            .lock()
            .unwrap()
            .clone()
            .ok_or(GrpcClientError::Call(tonic::Status::not_found("Round1 package not set")))
    }

    async fn get_round1_dkg_packages(&mut self, request: Empty) -> Result<DkgPayload, GrpcClientError> {
        Ok(DkgPayload::default())
    }

    async fn new_round1_dkg_package(&mut self, request: DkgPayload) -> Result<Empty, GrpcClientError> {
        Ok(Empty {})
    }

    async fn get_round2_dkg_package(&mut self, request: Empty) -> Result<DkgPayload, GrpcClientError> {
        self.round2_dkg_package
            .lock()
            .unwrap()
            .clone()
            .ok_or(GrpcClientError::Call(tonic::Status::not_found("Round2 package not set")))
    }

    async fn new_round2_dkg_package(&mut self, request: DkgPayload) -> Result<Empty, GrpcClientError> {
        Ok(Empty {})
    }

    async fn get_round1_signing_package(
        &mut self,
        request: SigningPackageRequest,
    ) -> Result<SigningPackage, GrpcClientError> {
        Ok(SigningPackage::default())
    }

    async fn get_round2_signing_package(
        &mut self,
        request: SigningPackageRequest,
    ) -> Result<SigningPackage, GrpcClientError> {
        Ok(SigningPackage::default())
    }

    async fn new_round1_signing_package(&mut self, request: SigningPackage) -> Result<Empty, GrpcClientError> {
        Ok(Empty {})
    }

    async fn get_psbt(&mut self, request: MakeTxRequest) -> Result<SigningPackage, GrpcClientError> {
        Ok(SigningPackage::default())
    }

    async fn get_to_sign_package(&mut self, request: ToSignRequest) -> Result<SigningPackage, GrpcClientError> {
        Ok(SigningPackage::default())
    }

    async fn new_round2_signing_package(&mut self, request: SigningPackage) -> Result<Empty, GrpcClientError> {
        Ok(Empty {})
    }

    async fn finalize_signing(
        &mut self,
        request: FinalizeSigningRequest,
    ) -> Result<FinalizeSigningResponse, GrpcClientError> {
        Ok(FinalizeSigningResponse::default())
    }

    async fn signer_finalize(
        &mut self,
        request: FinalizeSignerRequest,
    ) -> Result<FinalizeSigningResponse, GrpcClientError> {
        Ok(FinalizeSigningResponse::default())
    }

    async fn get_wallet_state(&mut self, request: Empty) -> Result<WalletStateResponse, GrpcClientError> {
        Ok(WalletStateResponse::default())
    }

    async fn abort_signing(&mut self, request: Empty) -> Result<Empty, GrpcClientError> {
        Ok(Empty {})
    }

    async fn get_signing_status(
        &mut self,
        request: GetSigningStatusRequest,
    ) -> Result<GetSigningStatusResponse, GrpcClientError> {
        Ok(GetSigningStatusResponse::default())
    }

    async fn get_session_ids(
        &mut self,
        request: GetSessionIdsRequest,
    ) -> Result<GetSessionIdsResponse, GrpcClientError> {
        Ok(GetSessionIdsResponse::default())
    }

    async fn health_check(&mut self, request: Empty) -> Result<Empty, GrpcClientError> {
        Ok(Empty {})
    }

    async fn tx_index_new_checkpoint(&mut self, request: SyncTxIndexRequest) -> Result<Empty, GrpcClientError> {
        Ok(Empty {})
    }

    async fn reset_all_utxos(&mut self, request: ResetAllUtxosRequest) -> Result<Empty, GrpcClientError> {
        Ok(Empty {})
    }

    async fn get_all_utxos(&mut self, request: Empty) -> Result<GetAllUtxosResponse, GrpcClientError> {
        Ok(GetAllUtxosResponse::default())
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MockFrostHandle {
    pub connected_peers: Arc<Mutex<HashMap<PeerId, PeerData>>>,
    pub check_connected_to_all: Arc<Mutex<bool>>,
}

impl MockFrostHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_connected_peers(mut self, peers: HashMap<PeerId, PeerData>) -> Self {
        *self.connected_peers.lock().unwrap() = peers;
        self
    }

    pub fn with_check_connected_to_all(mut self, connected: bool) -> Self {
        *self.check_connected_to_all.lock().unwrap() = connected;
        self
    }
}

impl ToFrostManager for MockFrostHandle {
    fn send_command(&self, cmd: FrostCommand) -> Result<(), SendError<FrostCommand>> {
        match cmd {
            FrostCommand::CheckConnectedToAll(tx) => {
                let is_connected = *self.check_connected_to_all.lock().unwrap();
                let _ = tx.send(is_connected);
            }
            FrostCommand::GetAllConnectedPeers(tx) => {
                let peers = self.connected_peers.lock().unwrap().clone();
                let _ = tx.send(peers);
            }
            FrostCommand::GetPeerMessagesStream(tx) => {
                let (peer_tx, peer_rx) = mpsc::unbounded_channel();
                let _ = tx.send(peer_rx);
            }
            _ => {}
        }
        Ok(())
    }
}

// Helper function to create a mock DkgPayload
pub(crate) fn create_mock_dkg_payload(identifier: Vec<u8>, payload: Vec<u8>) -> DkgPayload {
    DkgPayload { identifier, payload }
}

// Helper function to create a mock public key response  
pub(crate) fn create_mock_pubkey_response(pubkey: String) -> GetPublicKeyResponse {
    GetPublicKeyResponse { publickey: pubkey }
}

// ...existing mock implementations...
/// Creates a test storage instance with mock parameters
pub(crate) fn create_test_storage<EF, BF, DB: Clone>(
    genesis_authorities: Vec<secp256k1::PublicKey>,
    signer_index: usize,
    authority: secp256k1::PublicKey,
    aggregate_public_key: Option<secp256k1::PublicKey>,
    authority_socket_addresses: Vec<SocketAddr>,
    client: DB,
    bitcoind_factory: BF,
    executor_factory: EF,
) -> Storage<EF, BF, DB> {
    Storage::new(
        genesis_authorities,
        signer_index,
        authority,
        bitcoin::Network::Regtest,
        aggregate_public_key, 
        authority_socket_addresses,
        EthEvmConfig::default(),
        Arc::new(ChainSpec::default()),
        bitcoind_factory,
        executor_factory,
        client,
    )
}

/// Helper function to create authority socket addresses for testing
pub(crate) fn create_test_authority_addresses(num_authorities: usize) -> Vec<SocketAddr> {
    (0..num_authorities)
        .map(|i| format!("127.0.0.1:{}", 50000 + i).parse().unwrap())
        .collect()
}

/// Helper to create test authority keys
pub(crate) fn create_test_authority_keys(num_authorities: usize) -> Vec<secp256k1::PublicKey> {
    (0..num_authorities)
        .map(|i| {
            let secret_key = secp256k1::SecretKey::from_slice(&[i as u8 + 1; 32]).unwrap();
            secret_key.public_key(secp256k1::SECP256K1)
        })
        .collect()
}

/// Create an authority by index
pub(crate) fn create_test_authority(index: usize) -> secp256k1::PublicKey {
    let secret_key = secp256k1::SecretKey::from_slice(&[index as u8 + 1; 32]).unwrap();
    secret_key.public_key(secp256k1::SECP256K1)
}
