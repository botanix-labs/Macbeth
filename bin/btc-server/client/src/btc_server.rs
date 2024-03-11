#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ScriptBuf {
    /// Represents the Vec<u8> in Rust
    #[prost(bytes = "vec", tag = "1")]
    pub script: ::prost::alloc::vec::Vec<u8>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TxOut {
    /// / The value of the output, in satoshis.
    #[prost(uint64, tag = "1")]
    pub value: u64,
    /// / The script which must be satisfied for the output to be spent.
    #[prost(message, optional, tag = "2")]
    pub script_pubkey: ::core::option::Option<ScriptBuf>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct OutPoint {
    #[prost(bytes = "vec", tag = "1")]
    pub txid: ::prost::alloc::vec::Vec<u8>,
    #[prost(uint32, tag = "2")]
    pub vout: u32,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Utxo {
    #[prost(message, optional, tag = "1")]
    pub outpoint: ::core::option::Option<OutPoint>,
    #[prost(uint32, tag = "2")]
    pub output: u32,
    #[prost(string, tag = "3")]
    pub eth_address: ::prost::alloc::string::String,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GetAllUtxosResponse {
    #[prost(message, repeated, tag = "1")]
    pub utxos: ::prost::alloc::vec::Vec<Utxo>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RemoveUtxoRequest {
    /// The txid of the UTXO to remove.
    #[prost(string, tag = "1")]
    pub txid: ::prost::alloc::string::String,
    /// The output index of the UTXO to remove.
    #[prost(uint32, tag = "2")]
    pub vout: u32,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RemoveUtxoResponse {
    /// Indicates if the UTXO was successfully removed.
    #[prost(bool, tag = "1")]
    pub success: bool,
    /// Optional message with details about the operation.
    #[prost(string, tag = "2")]
    pub message: ::prost::alloc::string::String,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct NotifyPeginRequest {
    /// The txid of the utxo in hex.
    #[prost(string, tag = "1")]
    pub utxo_txid: ::prost::alloc::string::String,
    /// The output index of the utxo.
    #[prost(uint32, tag = "2")]
    pub utxo_vout: u32,
    /// The user's ethereum address.
    #[prost(string, tag = "3")]
    pub eth_address: ::prost::alloc::string::String,
    /// The txout of the utxo.
    #[prost(bytes = "vec", tag = "4")]
    pub output: ::prost::alloc::vec::Vec<u8>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GetGatewayAddressRequest {
    /// Eth address to tweak by
    #[prost(string, tag = "1")]
    pub eth_address: ::prost::alloc::string::String,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct MakeTxResponse {
    #[prost(string, tag = "1")]
    pub txid: ::prost::alloc::string::String,
    #[prost(bytes = "vec", tag = "2")]
    pub tx: ::prost::alloc::vec::Vec<u8>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GetPublicKeyResponse {
    /// hex encoded
    #[prost(string, tag = "1")]
    pub publickey: ::prost::alloc::string::String,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GetGatewayAddressResponse {
    /// hex encoded
    #[prost(string, tag = "1")]
    pub publickey: ::prost::alloc::string::String,
    #[prost(string, tag = "2")]
    pub tweaked_public_key: ::prost::alloc::string::String,
    #[prost(string, tag = "3")]
    pub gateway_address: ::prost::alloc::string::String,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Empty {}
/// Frost things
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct DkgPayload {
    #[prost(bytes = "vec", tag = "1")]
    pub identifier: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: ::prost::alloc::vec::Vec<u8>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Round1SigningPackageRequest {
    #[prost(bytes = "vec", tag = "1")]
    pub psbt: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub signing_session_id: ::prost::alloc::vec::Vec<u8>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Round1SigningPackage {
    #[prost(bytes = "vec", tag = "1")]
    pub identifier: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub psbt: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub signing_session_id: ::prost::alloc::vec::Vec<u8>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Round2SigningPackage {
    #[prost(bytes = "vec", tag = "1")]
    pub psbt: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub identifier: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub signing_session_id: ::prost::alloc::vec::Vec<u8>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SignPayload {
    #[prost(bytes = "vec", tag = "1")]
    pub psbt: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub signing_session_id: ::prost::alloc::vec::Vec<u8>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Output {
    #[prost(string, tag = "1")]
    pub address: ::prost::alloc::string::String,
    #[prost(uint64, tag = "2")]
    pub value: u64,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct MakeTxRequest {
    #[prost(message, repeated, tag = "1")]
    pub outputs: ::prost::alloc::vec::Vec<Output>,
    /// Fee rate in satoshi per vbyte.
    #[prost(uint32, tag = "2")]
    pub fee_rate: u32,
    #[prost(bytes = "vec", tag = "3")]
    pub signing_session_id: ::prost::alloc::vec::Vec<u8>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ToSignRequest {
    #[prost(bytes = "vec", tag = "3")]
    pub signing_session_id: ::prost::alloc::vec::Vec<u8>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct FinalizeSigningRequest {
    #[prost(bytes = "vec", tag = "1")]
    pub signing_session_id: ::prost::alloc::vec::Vec<u8>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct FinalizeSigningResponse {
    /// Finalized tx which includes witness data
    #[prost(bytes = "vec", tag = "1")]
    pub transaction: ::prost::alloc::vec::Vec<u8>,
}
/// Generated client implementations.
pub mod btc_server_client {
    #![allow(unused_variables, dead_code, missing_docs, clippy::let_unit_value)]
    use tonic::codegen::*;
    use tonic::codegen::http::Uri; 
    #[derive(Debug, Clone)]
    pub struct BtcServerClient<T> {
        inner: tonic::client::Grpc<T>,
    }
    impl BtcServerClient<tonic::transport::Channel> {
        /// Attempt to create a new client by connecting to a given endpoint.
        pub async fn connect<D>(dst: D) -> Result<Self, tonic::transport::Error>
        where
            D: TryInto<tonic::transport::Endpoint>,
            D::Error: Into<StdError>,
        {
            let conn = tonic::transport::Endpoint::new(dst)?.connect().await?;
            Ok(Self::new(conn))
        }
    }
    impl<T> BtcServerClient<T>
    where
        T: tonic::client::GrpcService<tonic::body::BoxBody>,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + Send + 'static,
        <T::ResponseBody as Body>::Error: Into<StdError> + Send,
    {
        pub fn new(inner: T) -> Self {
            let inner = tonic::client::Grpc::new(inner);
            Self { inner }
        }
        pub fn with_origin(inner: T, origin: Uri) -> Self {
            let inner = tonic::client::Grpc::with_origin(inner, origin);
            Self { inner }
        }
        pub fn with_interceptor<F>(
            inner: T,
            interceptor: F,
        ) -> BtcServerClient<InterceptedService<T, F>>
        where
            F: tonic::service::Interceptor,
            T::ResponseBody: Default,
            T: tonic::codegen::Service<
                http::Request<tonic::body::BoxBody>,
                Response = http::Response<
                    <T as tonic::client::GrpcService<tonic::body::BoxBody>>::ResponseBody,
                >,
            >,
            <T as tonic::codegen::Service<
                http::Request<tonic::body::BoxBody>,
            >>::Error: Into<StdError> + Send + Sync,
        {
            BtcServerClient::new(InterceptedService::new(inner, interceptor))
        }
        /// Compress requests with the given encoding.
        ///
        /// This requires the server to support it otherwise it might respond with an
        /// error.
        #[must_use]
        pub fn send_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.send_compressed(encoding);
            self
        }
        /// Enable decompressing responses.
        #[must_use]
        pub fn accept_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.accept_compressed(encoding);
            self
        }
        /// Limits the maximum size of a decoded message.
        ///
        /// Default: `4MB`
        #[must_use]
        pub fn max_decoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_decoding_message_size(limit);
            self
        }
        /// Limits the maximum size of an encoded message.
        ///
        /// Default: `usize::MAX`
        #[must_use]
        pub fn max_encoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_encoding_message_size(limit);
            self
        }
        pub async fn notify_pegin(
            &mut self,
            request: impl tonic::IntoRequest<super::NotifyPeginRequest>,
        ) -> std::result::Result<tonic::Response<super::Empty>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/NotifyPegin",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("btc_server.BtcServer", "NotifyPegin"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_gateway_address(
            &mut self,
            request: impl tonic::IntoRequest<super::GetGatewayAddressRequest>,
        ) -> std::result::Result<
            tonic::Response<super::GetGatewayAddressResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/GetGatewayAddress",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("btc_server.BtcServer", "GetGatewayAddress"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_public_key(
            &mut self,
            request: impl tonic::IntoRequest<super::Empty>,
        ) -> std::result::Result<
            tonic::Response<super::GetPublicKeyResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/GetPublicKey",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("btc_server.BtcServer", "GetPublicKey"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_round1_dkg_package(
            &mut self,
            request: impl tonic::IntoRequest<super::Empty>,
        ) -> std::result::Result<tonic::Response<super::DkgPayload>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/GetRound1DkgPackage",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("btc_server.BtcServer", "GetRound1DkgPackage"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_round1_dkg_packages(
            &mut self,
            request: impl tonic::IntoRequest<super::Empty>,
        ) -> std::result::Result<tonic::Response<super::DkgPayload>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/GetRound1DkgPackages",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("btc_server.BtcServer", "GetRound1DkgPackages"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn new_round1_dkg_package(
            &mut self,
            request: impl tonic::IntoRequest<super::DkgPayload>,
        ) -> std::result::Result<tonic::Response<super::Empty>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/NewRound1DkgPackage",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("btc_server.BtcServer", "NewRound1DkgPackage"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_round2_dkg_package(
            &mut self,
            request: impl tonic::IntoRequest<super::Empty>,
        ) -> std::result::Result<tonic::Response<super::DkgPayload>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/GetRound2DkgPackage",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("btc_server.BtcServer", "GetRound2DkgPackage"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn new_round2_dkg_package(
            &mut self,
            request: impl tonic::IntoRequest<super::DkgPayload>,
        ) -> std::result::Result<tonic::Response<super::Empty>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/NewRound2DkgPackage",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("btc_server.BtcServer", "NewRound2DkgPackage"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_round1_signing_package(
            &mut self,
            request: impl tonic::IntoRequest<super::Round1SigningPackageRequest>,
        ) -> std::result::Result<
            tonic::Response<super::Round1SigningPackage>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/GetRound1SigningPackage",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("btc_server.BtcServer", "GetRound1SigningPackage"),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_round2_signing_package(
            &mut self,
            request: impl tonic::IntoRequest<super::SignPayload>,
        ) -> std::result::Result<
            tonic::Response<super::Round2SigningPackage>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/GetRound2SigningPackage",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("btc_server.BtcServer", "GetRound2SigningPackage"),
                );
            self.inner.unary(req, path, codec).await
        }
        /// only meant to be used by the cordinator
        pub async fn new_round1_signing_package(
            &mut self,
            request: impl tonic::IntoRequest<super::Round1SigningPackage>,
        ) -> std::result::Result<tonic::Response<super::Empty>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/NewRound1SigningPackage",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("btc_server.BtcServer", "NewRound1SigningPackage"),
                );
            self.inner.unary(req, path, codec).await
        }
        /// Meant to be used at anytime to perform utxo selection and create a tx
        pub async fn get_psbt(
            &mut self,
            request: impl tonic::IntoRequest<super::MakeTxRequest>,
        ) -> std::result::Result<tonic::Response<super::SignPayload>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/GetPsbt",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("btc_server.BtcServer", "GetPsbt"));
            self.inner.unary(req, path, codec).await
        }
        /// Meant to be used to transition the signing round to round 2 after round 1
        /// signing commitments have been collected
        pub async fn get_to_sign_package(
            &mut self,
            request: impl tonic::IntoRequest<super::ToSignRequest>,
        ) -> std::result::Result<tonic::Response<super::SignPayload>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/GetToSignPackage",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("btc_server.BtcServer", "GetToSignPackage"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn new_round2_signing_package(
            &mut self,
            request: impl tonic::IntoRequest<super::Round2SigningPackage>,
        ) -> std::result::Result<tonic::Response<super::Empty>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/NewRound2SigningPackage",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("btc_server.BtcServer", "NewRound2SigningPackage"),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn finalize_signing(
            &mut self,
            request: impl tonic::IntoRequest<super::FinalizeSigningRequest>,
        ) -> std::result::Result<
            tonic::Response<super::FinalizeSigningResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/FinalizeSigning",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("btc_server.BtcServer", "FinalizeSigning"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_all_utxos(
            &mut self,
            request: impl tonic::IntoRequest<super::Empty>,
        ) -> std::result::Result<
            tonic::Response<super::GetAllUtxosResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/GetAllUtxos",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("btc_server.BtcServer", "GetAllUtxos"));
            self.inner.unary(req, path, codec).await
        }
        /// Update the RemoveUtxo method to return the new response type
        pub async fn remove_utxo(
            &mut self,
            request: impl tonic::IntoRequest<super::RemoveUtxoRequest>,
        ) -> std::result::Result<
            tonic::Response<super::RemoveUtxoResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::new(
                        tonic::Code::Unknown,
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/btc_server.BtcServer/RemoveUtxo",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("btc_server.BtcServer", "RemoveUtxo"));
            self.inner.unary(req, path, codec).await
        }
    }
}
