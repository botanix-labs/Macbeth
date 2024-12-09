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
