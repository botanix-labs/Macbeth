//! Extended bitcoin server client with authentication
use displaydoc::Display as DisplayDoc;
use reth_rpc::{Claims, JwtSecret};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tonic::metadata::{BinaryMetadataKey, MetadataValue};

use client::{
    BtcServerClient, DkgPayload, Empty, FinalizeSignerRequest, FinalizeSigningRequest,
    FinalizeSigningResponse, GetGatewayAddressRequest, GetGatewayAddressResponse,
    GetPublicKeyResponse, GetUtxoMerkleRootResponse, MakeTxRequest, NotifyPeginRequest,
    SigningPackage, SigningPackageRequest, ToSignRequest,
};

const JWT_HEADER_KEY: &str = "trace-proto-bin";

fn to_u64(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// grpc-related errors
#[non_exhaustive]
#[derive(Debug, DisplayDoc, Error)]
pub enum GrpcClientError {
    /// grpc transport error: `{0}`
    Transport(tonic::transport::Error),
    /// grpc call error: `{0}`
    Call(tonic::Status),
}

impl GrpcClientError {
    /// Maps to a tonic status code
    pub fn to_tonic_status(self) -> tonic::Status {
        match self {
            Self::Transport(e) => tonic::Status::internal(e.to_string()),
            Self::Call(e) => e,
        }
    }
}

/// Macro for generating grpc methods
macro_rules! generate_method {
    ($method_name:ident, $req_ty:ty, $resp_ty:ty) => {
        /// A general template for a grpc method receiving a request and returning a response
        pub async fn $method_name(
            &mut self,
            request: $req_ty,
        ) -> Result<$resp_ty, GrpcClientError> {
            let mut req = tonic::Request::new(request);

            // Insert JWT auth token if available
            if let Some(jwt_auth_token) = self.generate_jwt_token() {
                let jwt_auth_token = MetadataValue::from_bytes(jwt_auth_token.as_bytes());
                let key = BinaryMetadataKey::from_static(JWT_HEADER_KEY);
                req.metadata_mut().insert_bin(key, jwt_auth_token);
            }

            // Perform the gRPC call and handle the response
            match self.client.$method_name(req).await {
                Ok(response) => Ok(response.into_inner()),
                Err(status) => Err(GrpcClientError::Call(status)),
            }
        }
    };
}

/// Bitcoin Server Client with extended authentication credentials
#[derive(Clone, Debug)]
pub struct BtcServerExtendedClient {
    client: BtcServerClient<tonic::transport::channel::Channel>,
    jwt_secret: Option<JwtSecret>,
}

impl BtcServerExtendedClient {
    /// Create a new Bitcoin Server Client with extended authentication credentials
    pub async fn new(url: String, jwt_secret: Option<JwtSecret>) -> Result<Self, GrpcClientError> {
        let client = BtcServerClient::connect(url).await.map_err(GrpcClientError::Transport)?;

        Ok(Self { client, jwt_secret })
    }

    /// Updates the jwt secret
    pub fn update_jwt_secret(&mut self, jwt_secret: JwtSecret) {
        self.jwt_secret = Some(jwt_secret);
    }

    /// Generate a new jwt token from secret and claims
    /// TODO: fix unwraps
    pub fn generate_jwt_token(&mut self) -> Option<String> {
        self.jwt_secret.as_ref().map(|jwt_secret| {
            let claims = Claims { iat: to_u64(SystemTime::now()), exp: Some(10000000000) };
            let jwt_token = jwt_secret.encode(&claims).unwrap();
            jwt_secret.validate(jwt_token.clone()).unwrap();
            jwt_token
        })
    }

    generate_method!(notify_pegin, NotifyPeginRequest, Empty);
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
    generate_method!(get_utxo_merkle_root, Empty, GetUtxoMerkleRootResponse);
}

#[cfg(test)]
mod tests {
    use bitcoin::base64::decode;

    #[test]
    fn test_metadata_jwt_decode_encode() {
        use super::JWT_HEADER_KEY;
        use crate::extended_client::to_u64;
        use client::Empty;
        use reth_rpc::{Claims, JwtSecret};
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
            let jwt_token_base64_decoded = base64::decode(jwt_request_token_received).unwrap();

            let jwt_stringified = String::from_utf8(jwt_token_base64_decoded).unwrap();

            // validate the request token
            assert!(jwt_secret.validate(jwt_stringified).is_ok());
        }
    }
}
