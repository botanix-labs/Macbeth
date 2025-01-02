//! Extended bitcoin server client with authentication
use alloy_rpc_types_engine::{Claims, JwtSecret};
use client::{
    BtcServerClient, DkgPayload, Empty, FinalizeSignerRequest, FinalizeSigningRequest,
    FinalizeSigningResponse, GetAllUtxosResponse, GetGatewayAddressRequest,
    GetGatewayAddressResponse, GetPendingPegoutsResponse, GetPublicKeyResponse,
    GetSessionIdsRequest, GetSessionIdsResponse, GetSigningStatusRequest, GetSigningStatusResponse,
    GetTrackedTxsResponse, MakeTxRequest, NotifyPeginsRequest, NotifyPegoutsRequest,
    ResetAllUtxosRequest, ResetWalletStateRequest, SigningPackage, SigningPackageRequest,
    SyncTxIndexRequest, ToSignRequest, WalletStateResponse,
};
use displaydoc::Display as DisplayDoc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tonic::{
    metadata::{BinaryMetadataKey, MetadataValue},
    transport::Uri,
};
use futures_util::future::BoxFuture;

const JWT_HEADER_KEY: &str = "trace-proto-bin";

fn to_u64(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).expect("Duration since epoch cannot fail").as_secs()
}

/// grpc-related errors
#[non_exhaustive]
#[derive(Debug, DisplayDoc, Error)]
pub enum GrpcClientError {
    /// grpc transport error: `{0}`
    Transport(tonic::transport::Error),
    /// grpc call error: `{0}`
    Call(tonic::Status),
    /// invalid uri error: `{0}`
    InvalidUri(String),
}

impl GrpcClientError {
    /// Maps to a tonic status code
    pub fn to_tonic_status(self) -> tonic::Status {
        match self {
            Self::Transport(e) => tonic::Status::internal(e.to_string()),
            Self::Call(e) => e,
            Self::InvalidUri(e) => tonic::Status::internal(e),
        }
    }
}

pub trait BtcServerExtendedApi: Clone + Send + Sync + 'static {
    fn update_jwt_secret(&mut self, jwt_secret: JwtSecret);
    fn generate_jwt_token(&mut self) -> Option<String>;
    
    fn notify_pegins<'a>(&'a mut self, request: NotifyPeginsRequest) -> BoxFuture<'a, Result<Empty, GrpcClientError>>;
    fn notify_pegouts<'a>(&'a mut self, request: NotifyPegoutsRequest) -> BoxFuture<'a, Result<Empty, GrpcClientError>>;
    fn get_gateway_address<'a>(&'a mut self, request: GetGatewayAddressRequest) -> BoxFuture<'a, Result<GetGatewayAddressResponse, GrpcClientError>>;
    fn get_public_key<'a>(&'a mut self, request: Empty) -> BoxFuture<'a, Result<GetPublicKeyResponse, GrpcClientError>>;
    fn get_round1_dkg_package<'a>(&'a mut self, request: Empty) -> BoxFuture<'a, Result<DkgPayload, GrpcClientError>>;
    fn get_round1_dkg_packages<'a>(&'a mut self, request: Empty) -> BoxFuture<'a, Result<DkgPayload, GrpcClientError>>;
    fn new_round1_dkg_package<'a>(&'a mut self, request: DkgPayload) -> BoxFuture<'a, Result<Empty, GrpcClientError>>;
    fn get_round2_dkg_package<'a>(&'a mut self, request: Empty) -> BoxFuture<'a, Result<DkgPayload, GrpcClientError>>;
    fn new_round2_dkg_package<'a>(&'a mut self, request: DkgPayload) -> BoxFuture<'a, Result<Empty, GrpcClientError>>;
    fn get_round1_signing_package<'a>(&'a mut self, request: SigningPackageRequest) -> BoxFuture<'a, Result<SigningPackage, GrpcClientError>>;
    fn get_round2_signing_package<'a>(&'a mut self, request: SigningPackageRequest) -> BoxFuture<'a, Result<SigningPackage, GrpcClientError>>;
    fn new_round1_signing_package<'a>(&'a mut self, request: SigningPackage) -> BoxFuture<'a, Result<Empty, GrpcClientError>>;
    fn get_psbt<'a>(&'a mut self, request: MakeTxRequest) -> BoxFuture<'a, Result<SigningPackage, GrpcClientError>>;
    fn get_to_sign_package<'a>(&'a mut self, request: ToSignRequest) -> BoxFuture<'a, Result<SigningPackage, GrpcClientError>>;
    fn new_round2_signing_package<'a>(&'a mut self, request: SigningPackage) -> BoxFuture<'a, Result<Empty, GrpcClientError>>;
    fn finalize_signing<'a>(&'a mut self, request: FinalizeSigningRequest) -> BoxFuture<'a, Result<FinalizeSigningResponse, GrpcClientError>>;
    fn signer_finalize<'a>(&'a mut self, request: FinalizeSignerRequest) -> BoxFuture<'a, Result<FinalizeSigningResponse, GrpcClientError>>;
    fn get_wallet_state<'a>(&'a mut self, request: Empty) -> BoxFuture<'a, Result<WalletStateResponse, GrpcClientError>>;
    fn abort_signing<'a>(&'a mut self, request: Empty) -> BoxFuture<'a, Result<Empty, GrpcClientError>>;
    fn get_signing_status<'a>(&'a mut self, request: GetSigningStatusRequest) -> BoxFuture<'a, Result<GetSigningStatusResponse, GrpcClientError>>;
    fn get_session_ids<'a>(&'a mut self, request: GetSessionIdsRequest) -> BoxFuture<'a, Result<GetSessionIdsResponse, GrpcClientError>>;
    fn health_check<'a>(&'a mut self, request: Empty) -> BoxFuture<'a, Result<Empty, GrpcClientError>>;
    fn tx_index_new_checkpoint<'a>(&'a mut self, request: SyncTxIndexRequest) -> BoxFuture<'a, Result<Empty, GrpcClientError>>;
    fn reset_all_utxos<'a>(&'a mut self, request: ResetAllUtxosRequest) -> BoxFuture<'a, Result<Empty, GrpcClientError>>;
    fn get_all_utxos<'a>(&'a mut self, request: Empty) -> BoxFuture<'a, Result<GetAllUtxosResponse, GrpcClientError>>;
    fn get_tracked_txs<'a>(&'a mut self, request: Empty) -> BoxFuture<'a, Result<GetTrackedTxsResponse, GrpcClientError>>;
    fn get_pending_pegouts<'a>(&'a mut self, request: Empty) -> BoxFuture<'a, Result<GetPendingPegoutsResponse, GrpcClientError>>;
    fn reset_wallet_state<'a>(&'a mut self, request: ResetWalletStateRequest) -> BoxFuture<'a, Result<Empty, GrpcClientError>>;
}

/// Macro for generating grpc methods implementation
macro_rules! generate_method {
    ($method_name:ident, $req_ty:ty, $resp_ty:ty) => {
        fn $method_name<'a>(&'a mut self, request: $req_ty) -> BoxFuture<'a, Result<$resp_ty, GrpcClientError>> {
            Box::pin(async move {
                let mut req = tonic::Request::new(request);

                if let Some(jwt_auth_token) = self.generate_jwt_token() {
                    let jwt_auth_token = MetadataValue::from_bytes(jwt_auth_token.as_bytes());
                    let key = BinaryMetadataKey::from_static(JWT_HEADER_KEY);
                    req.metadata_mut().insert_bin(key, jwt_auth_token);
                }

                match self.client.$method_name(req).await {
                    Ok(response) => Ok(response.into_inner()),
                    Err(status) => Err(GrpcClientError::Call(status)),
                }
            })
        }
    };
}

/// Bitcoin Server Client implementation with extended authentication credentials
#[derive(Clone, Debug)]
pub struct BtcServerExtendedClient {
    client: BtcServerClient<tonic::transport::channel::Channel>,
    jwt_secret: Option<JwtSecret>,
}

impl BtcServerExtendedClient {
    /// Create a new Bitcoin Server Client with extended authentication credentials
    pub async fn new(url: String, jwt_secret: Option<JwtSecret>) -> Result<Self, GrpcClientError> {
        let uri = url.parse::<Uri>().map_err(|e| GrpcClientError::InvalidUri(e.to_string()))?;
        let chan = tonic::transport::Channel::builder(uri)
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(20))
            .http2_keep_alive_interval(Duration::from_secs(180))
            .tcp_nodelay(true)
            .keep_alive_while_idle(true);

        let client = BtcServerClient::connect(chan).await.map_err(GrpcClientError::Transport)?;

        Ok(Self { client, jwt_secret })
    }
}

impl BtcServerExtendedApi for BtcServerExtendedClient {
    fn update_jwt_secret(&mut self, jwt_secret: JwtSecret) {
        self.jwt_secret = Some(jwt_secret);
    }

    /// Generate a new jwt token from secret and claims
    /// TODO: fix unwraps
    fn generate_jwt_token(&mut self) -> Option<String> {
        self.jwt_secret.as_ref().map(|jwt_secret| {
            let claims = Claims { iat: to_u64(SystemTime::now()), exp: Some(10000000000) };
            let jwt_token = jwt_secret.encode(&claims).unwrap();
            jwt_secret.validate(&jwt_token.clone()).unwrap();
            jwt_token
        })
    }

    generate_method!(notify_pegins, NotifyPeginsRequest, Empty);
    generate_method!(notify_pegouts, NotifyPegoutsRequest, Empty);
    generate_method!(get_gateway_address, GetGatewayAddressRequest, GetGatewayAddressResponse);
    generate_method!(get_public_key, Empty, GetPublicKeyResponse);
    generate_method!(get_round1_dkg_package, Empty, DkgPayload);
    generate_method!(get_round1_dkg_packages, Empty, DkgPayload);
    generate_method!(new_round1_dkg_package, DkgPayload, Empty);
    generate_method!(get_round2_dkg_package, Empty, DkgPayload);
    generate_method!(new_round2_dkg_package, DkgPayload, Empty);
    generate_method!(get_round1_signing_package, SigningPackageRequest, SigningPackage);
    generate_method!(get_round2_signing_package, SigningPackageRequest, SigningPackage);
    generate_method!(new_round1_signing_package, SigningPackage, Empty);
    generate_method!(get_psbt, MakeTxRequest, SigningPackage);
    generate_method!(get_to_sign_package, ToSignRequest, SigningPackage);
    generate_method!(new_round2_signing_package, SigningPackage, Empty);
    generate_method!(finalize_signing, FinalizeSigningRequest, FinalizeSigningResponse);
    generate_method!(signer_finalize, FinalizeSignerRequest, FinalizeSigningResponse);
    generate_method!(get_wallet_state, Empty, WalletStateResponse);
    generate_method!(abort_signing, Empty, Empty);
    generate_method!(get_signing_status, GetSigningStatusRequest, GetSigningStatusResponse);
    generate_method!(get_session_ids, GetSessionIdsRequest, GetSessionIdsResponse);
    generate_method!(health_check, Empty, Empty);
    generate_method!(tx_index_new_checkpoint, SyncTxIndexRequest, Empty);
    generate_method!(reset_all_utxos, ResetAllUtxosRequest, Empty);
    generate_method!(get_all_utxos, Empty, GetAllUtxosResponse);
    generate_method!(get_tracked_txs, Empty, GetTrackedTxsResponse);
    generate_method!(get_pending_pegouts, Empty, GetPendingPegoutsResponse);
    generate_method!(reset_wallet_state, ResetWalletStateRequest, Empty);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrpcClientFactory {
    grpc_url: String,
    jwt_secret: Option<JwtSecret>,
}

impl GrpcClientFactory {
    pub fn new(grpc_url: String, jwt_secret: Option<JwtSecret>) -> Self {
        Self { grpc_url, jwt_secret }
    }

    pub async fn build_and_connect(&self) -> Result<BtcServerExtendedClient, GrpcClientError> {
        let client = BtcServerExtendedClient::new(self.grpc_url.clone(), self.jwt_secret).await?;

        Ok(client)
    }
}

#[cfg(test)]
mod tests {
    use alloy_rpc_types_engine::{Claims, JwtSecret};

    #[test]
    fn test_metadata_jwt_decode_encode() {
        use super::JWT_HEADER_KEY;
        use crate::extended_client::to_u64;
        use bitcoin::base64::{engine::general_purpose, Engine as _};
        use client::Empty;
        use std::time::SystemTime;
        use tonic::metadata::{BinaryMetadataKey, MetadataValue};
        // create a random jwt secret
        let jwt_secret = JwtSecret::random();

        // create jwt token using the secret
        let claims = Claims { iat: to_u64(SystemTime::now()), exp: Some(10000000000) };
        let jwt_token = jwt_secret.encode(&claims).unwrap();

        // encode and set the token as a metadata value
        let metadata_value = MetadataValue::from_bytes(jwt_token.as_bytes());

        // simulate sending a grpc request
        let key = BinaryMetadataKey::from_static(JWT_HEADER_KEY);
        let mut request = tonic::Request::new(Empty {});
        request.metadata_mut().insert_bin(key, metadata_value);

        // simulate reading the grpc request metadata
        let key = BinaryMetadataKey::from_static(JWT_HEADER_KEY);
        if let Some(metadata_value) = request.metadata().get_bin(key) {
            // try to verify the received token
            let jwt_request_token_received = metadata_value.as_encoded_bytes();
            let jwt_token_base64_decoded =
                general_purpose::STANDARD.decode(jwt_request_token_received).unwrap();

            let jwt_stringified = String::from_utf8(jwt_token_base64_decoded).unwrap();

            // validate the request token
            assert!(jwt_secret.validate(&jwt_stringified).is_ok());
        }
    }
}