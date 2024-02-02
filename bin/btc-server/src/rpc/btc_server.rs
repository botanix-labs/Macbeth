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
pub struct MakeTxRequest {
    #[prost(string, tag = "1")]
    pub address: ::prost::alloc::string::String,
    #[prost(uint64, tag = "2")]
    pub value: u64,
    /// Fee rate in satoshi per vbyte.
    #[prost(uint32, tag = "3")]
    pub fee_rate: u32,
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
pub struct Round1SigningPackage {
    #[prost(bytes = "vec", tag = "1")]
    pub identifier: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: ::prost::alloc::vec::Vec<u8>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SignPayload {
    #[prost(bytes = "vec", tag = "1")]
    pub psbt: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: ::prost::alloc::vec::Vec<u8>,
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
pub struct ToSignRequest {
    #[prost(message, repeated, tag = "1")]
    pub outputs: ::prost::alloc::vec::Vec<Output>,
    /// Fee rate in satoshi per vbyte.
    #[prost(uint32, tag = "2")]
    pub fee_rate: u32,
}
/// Generated server implementations.
pub mod btc_server_server {
    #![allow(unused_variables, dead_code, missing_docs, clippy::let_unit_value)]
    use tonic::codegen::*;
    /// Generated trait containing gRPC methods that should be implemented for use with BtcServerServer.
    #[async_trait]
    pub trait BtcServer: Send + Sync + 'static {
        async fn notify_pegin(
            &self,
            request: tonic::Request<super::NotifyPeginRequest>,
        ) -> std::result::Result<tonic::Response<super::Empty>, tonic::Status>;
        async fn get_public_key(
            &self,
            request: tonic::Request<super::Empty>,
        ) -> std::result::Result<
            tonic::Response<super::GetPublicKeyResponse>,
            tonic::Status,
        >;
        async fn get_round1_dkg_package(
            &self,
            request: tonic::Request<super::Empty>,
        ) -> std::result::Result<tonic::Response<super::DkgPayload>, tonic::Status>;
        async fn get_round1_dkg_packages(
            &self,
            request: tonic::Request<super::Empty>,
        ) -> std::result::Result<tonic::Response<super::DkgPayload>, tonic::Status>;
        async fn new_round1_dkg_package(
            &self,
            request: tonic::Request<super::DkgPayload>,
        ) -> std::result::Result<tonic::Response<super::Empty>, tonic::Status>;
        async fn get_round2_dkg_package(
            &self,
            request: tonic::Request<super::Empty>,
        ) -> std::result::Result<tonic::Response<super::DkgPayload>, tonic::Status>;
        async fn new_round2_dkg_package(
            &self,
            request: tonic::Request<super::DkgPayload>,
        ) -> std::result::Result<tonic::Response<super::Empty>, tonic::Status>;
        async fn get_round1_signing_package(
            &self,
            request: tonic::Request<super::Empty>,
        ) -> std::result::Result<
            tonic::Response<super::Round1SigningPackage>,
            tonic::Status,
        >;
        async fn get_round2_signing_package(
            &self,
            request: tonic::Request<super::SignPayload>,
        ) -> std::result::Result<
            tonic::Response<super::Round1SigningPackage>,
            tonic::Status,
        >;
        /// only meant to be used by the cordinator
        async fn new_round1_signing_package(
            &self,
            request: tonic::Request<super::Round1SigningPackage>,
        ) -> std::result::Result<tonic::Response<super::Empty>, tonic::Status>;
        async fn get_to_sign_package(
            &self,
            request: tonic::Request<super::ToSignRequest>,
        ) -> std::result::Result<tonic::Response<super::SignPayload>, tonic::Status>;
    }
    #[derive(Debug)]
    pub struct BtcServerServer<T: BtcServer> {
        inner: _Inner<T>,
        accept_compression_encodings: EnabledCompressionEncodings,
        send_compression_encodings: EnabledCompressionEncodings,
        max_decoding_message_size: Option<usize>,
        max_encoding_message_size: Option<usize>,
    }
    struct _Inner<T>(Arc<T>);
    impl<T: BtcServer> BtcServerServer<T> {
        pub fn new(inner: T) -> Self {
            Self::from_arc(Arc::new(inner))
        }
        pub fn from_arc(inner: Arc<T>) -> Self {
            let inner = _Inner(inner);
            Self {
                inner,
                accept_compression_encodings: Default::default(),
                send_compression_encodings: Default::default(),
                max_decoding_message_size: None,
                max_encoding_message_size: None,
            }
        }
        pub fn with_interceptor<F>(
            inner: T,
            interceptor: F,
        ) -> InterceptedService<Self, F>
        where
            F: tonic::service::Interceptor,
        {
            InterceptedService::new(Self::new(inner), interceptor)
        }
        /// Enable decompressing requests with the given encoding.
        #[must_use]
        pub fn accept_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.accept_compression_encodings.enable(encoding);
            self
        }
        /// Compress responses with the given encoding, if the client supports it.
        #[must_use]
        pub fn send_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.send_compression_encodings.enable(encoding);
            self
        }
        /// Limits the maximum size of a decoded message.
        ///
        /// Default: `4MB`
        #[must_use]
        pub fn max_decoding_message_size(mut self, limit: usize) -> Self {
            self.max_decoding_message_size = Some(limit);
            self
        }
        /// Limits the maximum size of an encoded message.
        ///
        /// Default: `usize::MAX`
        #[must_use]
        pub fn max_encoding_message_size(mut self, limit: usize) -> Self {
            self.max_encoding_message_size = Some(limit);
            self
        }
    }
    impl<T, B> tonic::codegen::Service<http::Request<B>> for BtcServerServer<T>
    where
        T: BtcServer,
        B: Body + Send + 'static,
        B::Error: Into<StdError> + Send + 'static,
    {
        type Response = http::Response<tonic::body::BoxBody>;
        type Error = std::convert::Infallible;
        type Future = BoxFuture<Self::Response, Self::Error>;
        fn poll_ready(
            &mut self,
            _cx: &mut Context<'_>,
        ) -> Poll<std::result::Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, req: http::Request<B>) -> Self::Future {
            let inner = self.inner.clone();
            match req.uri().path() {
                "/btc_server.BtcServer/NotifyPegin" => {
                    #[allow(non_camel_case_types)]
                    struct NotifyPeginSvc<T: BtcServer>(pub Arc<T>);
                    impl<
                        T: BtcServer,
                    > tonic::server::UnaryService<super::NotifyPeginRequest>
                    for NotifyPeginSvc<T> {
                        type Response = super::Empty;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::NotifyPeginRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                (*inner).notify_pegin(request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let inner = inner.0;
                        let method = NotifyPeginSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/btc_server.BtcServer/GetPublicKey" => {
                    #[allow(non_camel_case_types)]
                    struct GetPublicKeySvc<T: BtcServer>(pub Arc<T>);
                    impl<T: BtcServer> tonic::server::UnaryService<super::Empty>
                    for GetPublicKeySvc<T> {
                        type Response = super::GetPublicKeyResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::Empty>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                (*inner).get_public_key(request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let inner = inner.0;
                        let method = GetPublicKeySvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/btc_server.BtcServer/GetRound1DkgPackage" => {
                    #[allow(non_camel_case_types)]
                    struct GetRound1DkgPackageSvc<T: BtcServer>(pub Arc<T>);
                    impl<T: BtcServer> tonic::server::UnaryService<super::Empty>
                    for GetRound1DkgPackageSvc<T> {
                        type Response = super::DkgPayload;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::Empty>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                (*inner).get_round1_dkg_package(request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let inner = inner.0;
                        let method = GetRound1DkgPackageSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/btc_server.BtcServer/GetRound1DkgPackages" => {
                    #[allow(non_camel_case_types)]
                    struct GetRound1DkgPackagesSvc<T: BtcServer>(pub Arc<T>);
                    impl<T: BtcServer> tonic::server::UnaryService<super::Empty>
                    for GetRound1DkgPackagesSvc<T> {
                        type Response = super::DkgPayload;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::Empty>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                (*inner).get_round1_dkg_packages(request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let inner = inner.0;
                        let method = GetRound1DkgPackagesSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/btc_server.BtcServer/NewRound1DkgPackage" => {
                    #[allow(non_camel_case_types)]
                    struct NewRound1DkgPackageSvc<T: BtcServer>(pub Arc<T>);
                    impl<T: BtcServer> tonic::server::UnaryService<super::DkgPayload>
                    for NewRound1DkgPackageSvc<T> {
                        type Response = super::Empty;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::DkgPayload>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                (*inner).new_round1_dkg_package(request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let inner = inner.0;
                        let method = NewRound1DkgPackageSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/btc_server.BtcServer/GetRound2DkgPackage" => {
                    #[allow(non_camel_case_types)]
                    struct GetRound2DkgPackageSvc<T: BtcServer>(pub Arc<T>);
                    impl<T: BtcServer> tonic::server::UnaryService<super::Empty>
                    for GetRound2DkgPackageSvc<T> {
                        type Response = super::DkgPayload;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::Empty>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                (*inner).get_round2_dkg_package(request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let inner = inner.0;
                        let method = GetRound2DkgPackageSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/btc_server.BtcServer/NewRound2DkgPackage" => {
                    #[allow(non_camel_case_types)]
                    struct NewRound2DkgPackageSvc<T: BtcServer>(pub Arc<T>);
                    impl<T: BtcServer> tonic::server::UnaryService<super::DkgPayload>
                    for NewRound2DkgPackageSvc<T> {
                        type Response = super::Empty;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::DkgPayload>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                (*inner).new_round2_dkg_package(request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let inner = inner.0;
                        let method = NewRound2DkgPackageSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/btc_server.BtcServer/GetRound1SigningPackage" => {
                    #[allow(non_camel_case_types)]
                    struct GetRound1SigningPackageSvc<T: BtcServer>(pub Arc<T>);
                    impl<T: BtcServer> tonic::server::UnaryService<super::Empty>
                    for GetRound1SigningPackageSvc<T> {
                        type Response = super::Round1SigningPackage;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::Empty>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                (*inner).get_round1_signing_package(request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let inner = inner.0;
                        let method = GetRound1SigningPackageSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/btc_server.BtcServer/GetRound2SigningPackage" => {
                    #[allow(non_camel_case_types)]
                    struct GetRound2SigningPackageSvc<T: BtcServer>(pub Arc<T>);
                    impl<T: BtcServer> tonic::server::UnaryService<super::SignPayload>
                    for GetRound2SigningPackageSvc<T> {
                        type Response = super::Round1SigningPackage;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::SignPayload>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                (*inner).get_round2_signing_package(request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let inner = inner.0;
                        let method = GetRound2SigningPackageSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/btc_server.BtcServer/NewRound1SigningPackage" => {
                    #[allow(non_camel_case_types)]
                    struct NewRound1SigningPackageSvc<T: BtcServer>(pub Arc<T>);
                    impl<
                        T: BtcServer,
                    > tonic::server::UnaryService<super::Round1SigningPackage>
                    for NewRound1SigningPackageSvc<T> {
                        type Response = super::Empty;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::Round1SigningPackage>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                (*inner).new_round1_signing_package(request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let inner = inner.0;
                        let method = NewRound1SigningPackageSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/btc_server.BtcServer/GetToSignPackage" => {
                    #[allow(non_camel_case_types)]
                    struct GetToSignPackageSvc<T: BtcServer>(pub Arc<T>);
                    impl<T: BtcServer> tonic::server::UnaryService<super::ToSignRequest>
                    for GetToSignPackageSvc<T> {
                        type Response = super::SignPayload;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::ToSignRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                (*inner).get_to_sign_package(request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let inner = inner.0;
                        let method = GetToSignPackageSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                _ => {
                    Box::pin(async move {
                        Ok(
                            http::Response::builder()
                                .status(200)
                                .header("grpc-status", "12")
                                .header("content-type", "application/grpc")
                                .body(empty_body())
                                .unwrap(),
                        )
                    })
                }
            }
        }
    }
    impl<T: BtcServer> Clone for BtcServerServer<T> {
        fn clone(&self) -> Self {
            let inner = self.inner.clone();
            Self {
                inner,
                accept_compression_encodings: self.accept_compression_encodings,
                send_compression_encodings: self.send_compression_encodings,
                max_decoding_message_size: self.max_decoding_message_size,
                max_encoding_message_size: self.max_encoding_message_size,
            }
        }
    }
    impl<T: BtcServer> Clone for _Inner<T> {
        fn clone(&self) -> Self {
            Self(Arc::clone(&self.0))
        }
    }
    impl<T: std::fmt::Debug> std::fmt::Debug for _Inner<T> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{:?}", self.0)
        }
    }
    impl<T: BtcServer> tonic::server::NamedService for BtcServerServer<T> {
        const NAME: &'static str = "btc_server.BtcServer";
    }
}
