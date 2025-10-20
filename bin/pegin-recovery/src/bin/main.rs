#[macro_use]
extern crate log;

use peginrecoverylib::rpc::pegin_recovery::{
    pegin_recovery_service_server::{PeginRecoveryService, PeginRecoveryServiceServer},
    Empty, FILE_DESCRIPTOR_SET,
};
use std::net::SocketAddr;
use tonic::{transport::Server, Request, Response, Status};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_PORT: u16 = 50052;

/// Main service implementation
#[derive(Debug, Default)]
pub struct PeginRecoveryServiceImpl {}

#[tonic::async_trait]
impl PeginRecoveryService for PeginRecoveryServiceImpl {
    async fn health_check(&self, _request: Request<Empty>) -> Result<Response<Empty>, Status> {
        info!("Health check requested");

        Ok(Response::new(Empty {}))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logger
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .filter_module("pegin_recovery", log::LevelFilter::Debug)
        .init();

    info!("Starting Pegin Recovery Service v{}", VERSION);

    // Configure service address
    let addr = SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT));
    info!("gRPC server listening on {}", addr);

    // Create service
    let service = PeginRecoveryServiceImpl::default();
    let svc = PeginRecoveryServiceServer::new(service);

    // Configure reflection (for grpcurl and similar tools)
    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(FILE_DESCRIPTOR_SET)
        .build_v1()?;

    // Start server
    Server::builder().add_service(svc).add_service(reflection_service).serve(addr).await?;

    Ok(())
}
