#[macro_use]
extern crate log;

use std::{
    fmt::Debug,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime},
};

use alloy_rpc_types_engine::{JwtError, JwtSecret};
use base64::{engine::general_purpose, Engine};
use bitcoin::{consensus::Decodable, Amount, BlockHash, Psbt, ScriptBuf, Transaction, TxOut};
use bitcoin_hashes::Hash;
use bitcoincore_rpc::{Auth, RpcApi};
use btc_server::btc_server_server::{BtcServer, BtcServerServer};
use btcserverlib::{
    badarg,
    config::{Config, Error as ConfigError},
    coordinator::{self, error::CoordinatorError},
    database,
    merkle::get_wallet_state_commitment,
    pegout_id::PegoutId,
    pegout_scheduler::{self, PegoutRequest},
    rpc,
    shutdown::{stop_signal, StopHandle},
    signer::{self, error::SigningRound1Error},
    util::{
        btc_per_kb_to_sat_per_vb, deserialize_frost_peer_id, get_available_utxos,
        get_pegin_confirmation_depth, parse_eth_address, parse_signing_session_id, ParsingError,
    },
    wallet::{
        self,
        address::{generate_taproot_address, generate_tweaked_public_key},
        psbt::PsbtExt,
        util::VerifyingKeyExt,
    },
};
use file_descriptor::FILE_DESCRIPTOR_SET;
use frost_secp256k1_tr as frost;
use futures_util::future::FutureExt;
use rand::thread_rng;
use thiserror::Error;
use tokio::sync::{oneshot, Mutex};
use tonic::{codegen::CompressionEncoding, metadata::BinaryMetadataKey, transport::Server};

use btcserverlib::config::{GrpcConfig, TomlConfig};

use btcserverlib::{
    http::{create_web_server, state::ServerState},
    pegout_scheduler::PegoutScheduler,
    rpc::*,
    signer::error::SigningError,
    telemetry::Telemetry,
};

const JWT_HEADER_KEY: &str = "trace-proto-bin";

macro_rules! already_exists {
    ($($arg:tt)*) => {{
        tonic::Status::already_exists(format!($($arg)*))
    }};
}

macro_rules! internal {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        error!("INTERNAL ERROR: {}", msg);
        tonic::Status::internal(format!("internal error: {}", msg))
    }};
}

macro_rules! unauthenticated {
    ($($arg:tt)*) => {{
        tonic::Status::unauthenticated(format!($($arg)*))
    }};
}

#[derive(Debug, Error)]
pub enum DKGError {
    #[error("Missing 1 dkg payload")]
    MissingRound1DkgPayload,
    #[error("Failed to get round 2 dkg payload")]
    FailedToGetRound2DkgPayload,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("Signing error")]
    Signing(#[from] SigningError),
    #[error("Coordination error")]
    Coordination(#[from] coordinator::error::CoordinatorError),
    #[error("Parsing error")]
    Parsing(#[from] ParsingError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("frost error: {0}")]
    Frost(frost_secp256k1_tr::Error),
    #[error("jwt error: {0}")]
    Jwt(#[from] JwtError),
    #[error("grpc reflection server error: {0}")]
    ReflectionServer(tonic_reflection::server::Error),
    #[error("db error: {0}")]
    Db(#[from] database::Error),
    #[error("sync error: {0}")]
    PegoutSchedulerSync(#[from] pegout_scheduler::SyncError),
    #[error("failed to sync to given checkpoint block: {0}")]
    FailedToReachCheckPoint(BlockHash),
    #[error("config error: {0}")]
    Config(#[from] ConfigError),
    #[error("dkg error: {0}")]
    Dkg(#[from] DKGError),
}

// To status util to convert Results with top level errors to tonic::Status
trait ToStatus<T> {
    fn to_status(self) -> Result<T, tonic::Status>;
}
impl<T, S: Into<Error> + Debug> ToStatus<T> for Result<T, S> {
    fn to_status(self) -> Result<T, tonic::Status> {
        match self {
            Ok(v) => Ok(v),
            Err(e) => match e.into() {
                Error::Signing(signing) => Err(internal!("Signing error: {}", signing)),
                Error::Config(internal) => Err(internal!("{:?}", internal)),
                Error::Frost(frost) => Err(internal!("Frost error: {}", frost)),
                Error::Coordination(coordination) => {
                    Err(internal!("Coordination error: {}", coordination))
                }
                Error::Parsing(parsing) => Err(internal!("Parsing error: {}", parsing)),
                Error::Io(io) => Err(internal!("Io error: {}", io)),
                Error::Jwt(jwt) => Err(internal!("Jwt error: {}", jwt)),
                Error::ReflectionServer(reflection_server) => {
                    Err(internal!("Reflection server error: {}", reflection_server))
                }
                Error::Db(db) => Err(internal!("Db error: {}", db)),
                Error::PegoutSchedulerSync(pegout_scheduler_sync) => {
                    Err(internal!("Pegout scheduler sync error: {}", pegout_scheduler_sync))
                }
                Error::FailedToReachCheckPoint(failed_to_reach_check_point) => {
                    Err(internal!("Failed to reach check point: {}", failed_to_reach_check_point))
                }
                Error::Dkg(dkg) => Err(internal!("Dkg error: {}", dkg)),
            },
        }
    }
}

impl<T> ToStatus<T> for Result<T, frost::Error> {
    fn to_status(self) -> Result<T, tonic::Status> {
        match self {
            Ok(v) => Ok(v),
            Err(e) => Err(internal!("Frost error: {}", e)),
        }
    }
}

impl<T> ToStatus<T> for Result<T, bitcoin::psbt::Error> {
    fn to_status(self) -> Result<T, tonic::Status> {
        match self {
            Ok(v) => Ok(v),
            Err(e) => Err(badarg!("Failed to parse PSBT: {}", e)),
        }
    }
}
impl<T> ToStatus<T> for Result<T, bitcoin::psbt::ExtractTxError> {
    fn to_status(self) -> Result<T, tonic::Status> {
        match self {
            Ok(v) => Ok(v),
            Err(e) => Err(badarg!("Failed to extract tx: {}", e)),
        }
    }
}
impl<T> ToStatus<T> for Result<T, hex::FromHexError> {
    fn to_status(self) -> Result<T, tonic::Status> {
        match self {
            Ok(v) => Ok(v),
            Err(e) => Err(badarg!("Failed to hex decode: {}", e)),
        }
    }
}
type SigningNoncesCommitmentsMap =
    Arc<Mutex<Option<Vec<(frost::round1::SigningNonces, frost::round1::SigningCommitments)>>>>;

struct App<BitcoinRpcApi> {
    db: database::Db,
    btc_network: bitcoin::Network,
    pegout_scheduler: Mutex<PegoutScheduler>,
    /// This lock is taken when we're making a tx so that we don't accidentally
    /// spend the same operations twice.
    tx_lock: Arc<Mutex<()>>,
    identifier: frost::Identifier,
    max_signers: u16,
    min_signers: u16,
    frost_round1_dkg: Arc<
        Mutex<Option<(frost::keys::dkg::round1::SecretPackage, frost::keys::dkg::round1::Package)>>,
    >,
    frost_round2_dkg: Arc<Mutex<Option<frost::keys::dkg::round2::SecretPackage>>>,
    /// The signing nonces for the current signing session
    /// We will replace this value in the case of a new signing session
    frost_round1_nonces: SigningNoncesCommitmentsMap,
    /// configuration
    config: Config,
    /// Btc signing server jwt secret
    btc_signing_server_jwt_secret: Option<JwtSecret>,
    /// bitcoind client
    bitcoind_client: BitcoinRpcApi,
    /// Fall back fee rate
    fall_back_fee_rate: bitcoin::FeeRate,
    /// telemetry
    telemetry: Option<Arc<Telemetry>>,
}

impl<BitcoindClient> App<BitcoindClient>
where
    BitcoindClient: RpcApi + Send + Sync + 'static,
{
    fn validate_jwt<T>(&self, request: &tonic::Request<T>) -> Result<(), tonic::Status> {
        let key = BinaryMetadataKey::from_static(JWT_HEADER_KEY);
        match (request.metadata().get_bin(key), self.btc_signing_server_jwt_secret.as_ref()) {
            (None, None) => {
                // we are in test mode, user has deliberately switched off authentication and is
                // making direct requests without jwt
                return Ok(());
            }
            (metadata_value, jwt_secret) => {
                match jwt_secret {
                    Some(jwt_secret) => {
                        // we have activated verification
                        if let Some(metadata_value) = metadata_value {
                            let jwt_request_token_received = metadata_value.as_encoded_bytes();
                            let jwt_token_base64_decoded = general_purpose::STANDARD
                                .decode(jwt_request_token_received)
                                .map_err(|e| {
                                    error!("Failed to base64 decode request metadata: {}", e);
                                    badarg!("Failed to base64 decode request metadata: {}", e)
                                })?;
                            let jwt_stringified = String::from_utf8(jwt_token_base64_decoded)
                                .map_err(|e| {
                                    error!("Failed to utf8 decode jwt value: {}", e);
                                    badarg!("Failed to utf8 decode jwt value: {}", e)
                                })?;
                            jwt_secret.validate(&jwt_stringified).map_err(|e| {
                                error!("Request authentication failed {}", e);
                                unauthenticated!("Request authentication failed")
                            })?;
                        } else {
                            error!("Missing JWT in request metadata. Warning: Btc-server cannot authenticate request!");
                            return Err(unauthenticated!("Missing JWT in request metadata. Warning: Btc-server cannot authenticate request!"));
                        }
                    }
                    None => {
                        warn!("Warning: btc server has no authentication activated. Request will be executed");
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    fn load_pegout_scheduler(
        db: &database::Db,
        fallback_checkpoint: BlockHash,
        pegin_conf_depth: u32,
    ) -> Result<PegoutScheduler, database::Error> {
        if let Some(latest) = db.get_pegout_mgr_finalized_block()? {
            let txs = db.get_tracked_txs()?;
            info!("Loaded pegout scheduler with {} pending txs", txs.len());
            Ok(PegoutScheduler::new(pegin_conf_depth, txs, latest, db.clone()))
        } else {
            info!("No finalized block found, using fallback checkpoint: {}", fallback_checkpoint);
            Ok(PegoutScheduler::new(pegin_conf_depth, vec![], fallback_checkpoint, db.clone()))
        }
    }

    fn get_or_create_jwt_secret_from_path(path: &Path) -> Result<JwtSecret, JwtError> {
        if path.exists() {
            JwtSecret::from_file(path)
        } else {
            JwtSecret::try_create_random(path)
        }
    }

    pub fn new(
        config: Config,
        bitcoind_client: BitcoindClient,
        telemetry: Option<Arc<Telemetry>>,
    ) -> Result<Self, Error> {
        let config = config.clone();
        let db = database::Db::open(&config.db).expect("failed to open db");

        // +1 b/c we use the index of the federation member's pk as the identifier
        // And 0 is not a valid identifier
        let frost_identifier =
            frost::Identifier::derive(config.identifier.to_le_bytes().as_slice())
                .expect("valid identifier");
        info!("Frost identifier: {:?} - {:?}", config.identifier, frost_identifier);

        let min_signers = config.min_signers;
        let max_signers = config.max_signers;
        if min_signers > max_signers {
            panic!("min_signers should be less than or equal to max_signers");
        }
        if min_signers < 2 {
            panic!("min_signers should be at least 2");
        }

        let mut btc_signing_server_jwt_secret = None;
        if let Some(btc_signing_server_jwt_path) = config.btc_signing_server_jwt_secret.as_ref() {
            btc_signing_server_jwt_secret = Some(
                Self::get_or_create_jwt_secret_from_path(btc_signing_server_jwt_path)
                    .map_err(Error::Jwt)?,
            )
        };

        let mut round1_dkg = None;
        if db.get_public_key_package().expect("failed to get public key package").is_none() {
            let rng = thread_rng();
            round1_dkg = Some(
                frost::keys::dkg::part1(frost_identifier, max_signers, min_signers, rng)
                    .map_err(Error::Frost)?,
            );
            info!("Successfully generated round 1 dkg: {:?}", round1_dkg);
        }

        let fall_back_fee_rate =
            bitcoin::FeeRate::from_sat_per_vb(config.fall_back_fee_rate_sat_per_vbyte)
                .expect("valid fee rate");

        let pegin_confirmation_depth = get_pegin_confirmation_depth(config.btc_network);
        let fallback_checkpoint = {
            let tip_height = bitcoind_client
                .get_block_count()
                .map_err(|e| Error::PegoutSchedulerSync(e.into()))?;
            bitcoind_client
                .get_block_hash(tip_height.saturating_sub(pegin_confirmation_depth as u64))
                .map_err(|e| Error::PegoutSchedulerSync(e.into()))?
        };
        let pegout_manager = Mutex::new(Self::load_pegout_scheduler(
            &db,
            fallback_checkpoint,
            pegin_confirmation_depth,
        )?);
        Ok(Self {
            btc_network: config.btc_network,
            db,
            pegout_scheduler: pegout_manager,
            tx_lock: Arc::new(Mutex::new(())),
            identifier: frost_identifier,
            frost_round1_dkg: Arc::new(Mutex::new(round1_dkg)),
            frost_round2_dkg: Arc::new(Mutex::new(None)),
            frost_round1_nonces: Arc::new(Mutex::new(None)),
            config,
            btc_signing_server_jwt_secret,
            min_signers,
            max_signers,
            bitcoind_client,
            fall_back_fee_rate,
            telemetry,
        })
    }

    pub async fn serve_async(self) -> Result<StopHandle, Error> {
        // init grpc config
        let grpc_config = if let Some(toml_config) = self.config.toml.as_ref() {
            TomlConfig::new(toml_config).await.map_err(Error::Config)?.grpc
        } else {
            GrpcConfig::default()
        };

        let (shutdown_send, shutdown_recv) = oneshot::channel::<()>();
        // create a server builder
        let mut server_builder = Server::builder()
            .concurrency_limit_per_connection(grpc_config.concurrency_limit_per_connection)
            .timeout(grpc_config.timeout)
            .initial_stream_window_size(grpc_config.initial_stream_window_size)
            .initial_connection_window_size(grpc_config.initial_connection_window_size)
            .max_concurrent_streams(grpc_config.max_concurrent_streams)
            .tcp_keepalive(grpc_config.tcp_keepalive)
            .tcp_nodelay(grpc_config.tcp_nodelay)
            .http2_keepalive_interval(grpc_config.http2_keepalive_interval)
            .http2_keepalive_timeout(grpc_config.http2_keepalive_timeout)
            .http2_adaptive_window(grpc_config.http2_adaptive_window)
            .max_frame_size(grpc_config.max_frame_size);

        // build the server
        let socket_addr: SocketAddr =
            self.config.address.clone().parse().expect("Unable to parse socket address");

        let mut btc_server = BtcServerServer::new(self)
            .max_decoding_message_size(grpc_config.max_decoding_message_size)
            .max_encoding_message_size(grpc_config.max_encoding_message_size);

        if let Some(encoding) = &grpc_config.send_compressed {
            if encoding.eq_ignore_ascii_case("Gzip") {
                btc_server = btc_server.send_compressed(CompressionEncoding::Gzip)
            }
        }

        if let Some(encoding) = &grpc_config.accept_compressed {
            if encoding.eq_ignore_ascii_case("Gzip") {
                btc_server = btc_server.accept_compressed(CompressionEncoding::Gzip);
            }
        }

        // now add the btc server to the builder
        let mut router = server_builder.add_service(btc_server);

        // add health service
        let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
        health_reporter.set_serving::<BtcServerServer<App<BitcoindClient>>>().await;

        let mut health_reporter = health_reporter.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                health_reporter.set_serving::<BtcServerServer<App<BitcoindClient>>>().await;
            }
        });

        // if reflection, add the reflection server to the builder
        if grpc_config.enable_reflection {
            let reflection_service = tonic_reflection::server::Builder::configure()
                .register_encoded_file_descriptor_set(FILE_DESCRIPTOR_SET)
                .build_v1()
                .map_err(Error::ReflectionServer)?;

            router = router.add_service(reflection_service);
            router = router.add_service(health_service);
        }

        // spawn the grpc server with a stop signal
        tokio::spawn(router.serve_with_shutdown(socket_addr, shutdown_recv.map(drop)));

        Ok(StopHandle { stop_cmd_sender: shutdown_send })
    }

    /// Sync the pegout scheduler to the given checkpoint.
    /// Typically the checkpoint will be a sufficiently deep block on L1.
    pub async fn sync_pegout_scheduler(
        &self,
        checkpoint: BlockHash,
    ) -> Result<(), pegout_scheduler::SyncError> {
        let mut lock = self.pegout_scheduler.lock().await;
        lock.sync_until(&self.bitcoind_client, checkpoint, self.telemetry.clone())?;
        self.db.store_pegout_mgr_finalized_block(lock.last_finalized())?;
        self.db.update_utxo_merkle_root()?;
        self.db.flush()?;
        Ok(())
    }

    /// Add a tracked transaction to the pegout scheduler.
    pub async fn add_tracked_tx(
        &self,
        tx: Transaction,
        targets: &[PegoutRequest],
        timestamp: SystemTime,
    ) -> Result<(), database::Error> {
        let mut txindex = self.pegout_scheduler.lock().await;
        let tx = txindex.add_tx(tx, targets, timestamp);
        self.db.store_tracked_tx(tx)?;
        self.db.flush()?;
        Ok(())
    }
}

#[tonic::async_trait]
impl<BitcoindClient> BtcServer for App<BitcoindClient>
where
    BitcoindClient: RpcApi + Send + Sync + 'static,
{
    /* General Endpoints */
    async fn health_check(
        &self,
        request: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&request)?;
        Ok(tonic::Response::new(rpc::Empty {}))
    }

    // unified interface to update btc-server and the pegout scheduler
    // TODO(scott): add light block field on the request
    async fn new_consensus_checkpoint(
        &self,
        request: tonic::Request<rpc::ConsensusCheckpointRequest>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&request)?;
        let req = request.into_inner();

        // sync the pegout scheduler to the given checkpoint
        let reader = &mut req.checkpoint_block_hash.as_slice();
        let checkpoint = bitcoin::BlockHash::consensus_decode(reader).map_err(|e| {
            error!("Failed to parse checkpoint hash: {}", e);
            badarg!("Failed to parse checkpoint hash: {}", e)
        })?;
        self.sync_pegout_scheduler(checkpoint).await.to_status()?;

        // process and store pegin utxos
        let utxos: Result<Vec<crate::database::Utxo>, _> =
            req.pegins.into_iter().map(TryFrom::try_from).collect();
        let utxos = utxos.map_err(|e| badarg!("Failed to parse utxos: {}", e))?;
        let utxo_refs: Vec<&crate::database::Utxo> = utxos.iter().collect();

        self.db.store_utxos(&utxo_refs).to_status()?;
        self.db.update_utxo_merkle_root().to_status()?;
        self.db.flush().to_status()?;
        info!("processed pegins.len(): {:?}", utxos.len());

        // process and store pending pegout requests
        let (available_utxos, tracked_inputs) = get_available_utxos(&self.db).await.to_status()?;
        if available_utxos.is_empty() && tracked_inputs.is_empty() {
            error!("Received a pegout request when there are no utxos or pending transactions");
            return Ok(tonic::Response::new(rpc::Empty {}));
        }
        let pegouts = req
            .pending_pegouts
            .into_iter()
            .map(|p| {
                let spk = ScriptBuf::from_bytes(p.spk);
                // basic sanity check over spk
                let _ = bitcoin::Address::from_script(&spk, self.btc_network).map_err(|e| {
                    error!(
                        "Failed to parse pegout spk for network: {:?}, error: {}",
                        self.btc_network, e
                    );
                    badarg!(
                        "Failed to parse pegout spk for network: {:?}, error: {}",
                        self.btc_network,
                        e
                    )
                })?;
                Ok(PegoutRequest {
                    id: PegoutId::from_bytes(&p.pegout_id).map_err(|_| {
                        error!("Failed to parse pegout id: {:?}", p.pegout_id);
                        badarg!("Failed to parse pegout id: {:?}", p.pegout_id)
                    })?,
                    spk,
                    value: Amount::from_sat(p.amount),
                    botanix_height: p.height,
                })
            })
            .collect::<Result<Vec<PegoutRequest>, tonic::Status>>();

        let pegouts = pegouts?;
        let pegouts_refs: Vec<&PegoutRequest> = pegouts.iter().collect();
        self.db.store_pending_pegouts(&pegouts_refs).to_status()?;
        self.db.flush().to_status()?;
        info!("stored pegouts.len(): {:?}", pegouts.len());

        Ok(tonic::Response::new(rpc::Empty {}))
    }

    async fn get_signing_status(
        &self,
        req: tonic::Request<rpc::GetSigningStatusRequest>,
    ) -> Result<tonic::Response<rpc::GetSigningStatusResponse>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let signing_session_id = parse_signing_session_id(&req.signing_session_id).to_status()?;
        let signing_status = self.db.get_signing_status(&signing_session_id).to_status()?;

        let res =
            tonic::Response::new(rpc::GetSigningStatusResponse { status: signing_status.into() });

        Ok(res)
    }

    async fn get_session_ids(
        &self,
        req: tonic::Request<rpc::GetSessionIdsRequest>,
    ) -> Result<tonic::Response<rpc::GetSessionIdsResponse>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let signing_session_ids = self.db.get_session_ids(req.max_results).to_status()?;

        let res = tonic::Response::new(rpc::GetSessionIdsResponse {
            data: signing_session_ids.into_iter().map(|s| s.to_vec()).collect(),
        });

        Ok(res)
    }

    /* Pegout Endpoints */
    async fn get_pending_pegouts(
        &self,
        req: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::GetPendingPegoutsResponse>, tonic::Status> {
        self.validate_jwt(&req)?;
        let pending_pegouts = self.db.get_pending_pegouts().to_status()?;
        if let Some(telemetry) = self.telemetry.as_ref() {
            telemetry.update_pending_pegouts(pending_pegouts.len() as i64);
        }
        let res = tonic::Response::new(rpc::GetPendingPegoutsResponse {
            pending_pegouts: pending_pegouts
                .into_iter()
                .map(|p| rpc::PendingPegout {
                    pegout_id: p.id.as_bytes().to_vec(),
                    spk: p.spk.into_bytes().to_vec(),
                    amount: p.value.to_sat(),
                    height: p.botanix_height,
                })
                .collect(),
        });
        Ok(res)
    }

    /* Wallet State Endpoints */
    /// Resets all utxos in the database
    async fn reset_all_utxos(
        &self,
        req: tonic::Request<rpc::ResetAllUtxosRequest>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        info!("Received reset all utxos request");
        let utxos: Result<Vec<crate::database::Utxo>, _> =
            req.utxos.into_iter().map(TryFrom::try_from).collect();
        let utxos = utxos.to_status()?;
        let utxo_refs: Vec<&crate::database::Utxo> = utxos.iter().collect();
        self.db.reset_utxos(&utxo_refs).to_status()?;
        Ok(tonic::Response::new(rpc::Empty {}))
    }

    async fn get_all_utxos(
        &self,
        req: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::GetAllUtxosResponse>, tonic::Status> {
        self.validate_jwt(&req)?;
        let db_utxos = self.db.get_all_utxos().to_status()?;
        let utxos = db_utxos
            .into_iter()
            .map(TryFrom::try_from)
            .collect::<Result<Vec<rpc::Utxo>, _>>()
            .map_err(|e| internal!("Failed to get utxos: {}", e))?;
        let res = rpc::GetAllUtxosResponse { utxos };

        Ok(tonic::Response::new(res))
    }

    // Gets the merkle root of the utxo set
    async fn get_wallet_state(
        &self,
        request: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::WalletStateResponse>, tonic::Status> {
        self.validate_jwt(&request)?;
        let wallet_state = get_wallet_state_commitment(&self.db).map_err(|e| {
            error!("Failed to get wallet state commitment: {}", e);
            internal!("Failed to get wallet state commitment: {}", e)
        })?;

        let res = rpc::WalletStateResponse {
            utxo_root: wallet_state.utxo_root,
            tracked_tx_root: wallet_state.tracked_tx_root,
            pending_pegouts_root: wallet_state.pending_pegouts_root,
            wallet_state_commitment: wallet_state.wallet_state_commitment,
        };
        Ok(tonic::Response::new(res))
    }

    // Get the tracked txs
    async fn get_tracked_txs(
        &self,
        request: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::GetTrackedTxsResponse>, tonic::Status> {
        self.validate_jwt(&request)?;
        let db_tracked_txs = self.db.get_tracked_txs().map_err(|e| {
            error!("Failed to get tracked txs: {}", e);
            internal!("Failed to get tracked txs: {}", e)
        })?;
        let tracked_txs = db_tracked_txs
            .into_iter()
            .map(TryFrom::try_from)
            .collect::<Result<Vec<rpc::TrackedTx>, _>>()
            .map_err(|_| internal!("Failed to convert tracked_tx"))?;

        let res = rpc::GetTrackedTxsResponse { tracked_txs };
        Ok(tonic::Response::new(res))
    }

    async fn reset_wallet_state(
        &self,
        _req: tonic::Request<rpc::ResetWalletStateRequest>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        panic!("not used yet");
        // self.validate_jwt(&req)?;
        // let req = req.into_inner();
        // info!("Received reset wallet state request");

        // // handle utxos
        // let utxos: Result<Vec<crate::database::Utxo>, _> =
        //     req.utxos.into_iter().map(TryFrom::try_from).collect();
        // let utxos = utxos.to_status()?;
        // let utxo_refs: Vec<&crate::database::Utxo> = utxos.iter().collect();
        // self.db.reset_utxos(&utxo_refs).to_status()?;

        // // handle tracked txs
        // let tracked_txs = req
        //     .tracked_txs
        //     .into_iter()
        //     .map(TryFrom::try_from)
        //     .collect::<Result<Vec<crate::pegout_scheduler::Tx>, _>>()
        //     .map_err(|e| internal!("Failed to convert tracked tx: {}", e))?;
        // let tracked_txs_refs: Vec<&crate::pegout_scheduler::Tx> = tracked_txs.iter().collect();
        // self.db.reset_tracked_txs(&tracked_txs_refs).to_status()?;

        // // handle pending pegouts
        // let pending_pegouts = req
        //     .pending_pegouts
        //     .into_iter()
        //     .map(TryFrom::try_from)
        //     .collect::<Result<Vec<PegoutRequest>, _>>()
        //     .map_err(|e| internal!("Failed to convert pending pegout: {}", e))?;
        // let pending_pegouts_refs: Vec<&PegoutRequest> = pending_pegouts.iter().collect();
        // self.db.reset_pending_pegouts(&pending_pegouts_refs).to_status()?;
    }

    /* Signer Endpoints */
    async fn abort_signing(
        &self,
        _req: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.db.get_key_package().to_status()?;

        // Clear the signing nonces
        let mut nonces_lock = self.frost_round1_nonces.lock().await;
        nonces_lock.take();
        assert!(nonces_lock.is_none());

        if let Some(telemetry) = self.telemetry.as_ref() {
            telemetry.record_aborted_signing_sessions(self.btc_network, self.config.identifier);
        }

        Ok(tonic::Response::new(rpc::Empty {}))
    }
    /// Endpoint responds with a nonce commitments for a ONE particular signings session
    async fn get_round1_signing_package(
        &self,
        req: tonic::Request<rpc::SigningPackageRequest>,
    ) -> Result<tonic::Response<rpc::SigningPackage>, tonic::Status> {
        self.validate_jwt(&req)?;
        // Ensure we have a key package
        self.db.get_key_package().to_status()?;

        let req = req.into_inner();
        info!(
            "Received round1 signing package request for signing session id: {:?}",
            hex::encode(req.signing_session_id.clone())
        );
        let signing_session_id = parse_signing_session_id(&req.signing_session_id).to_status()?;

        // Check if we have already provided nonces for the current session
        let mut nonces_lock = self.frost_round1_nonces.lock().await;
        if nonces_lock.is_some() {
            if let Some(telemetry) = self.telemetry.as_ref() {
                telemetry.update_signing_error_metrics(
                    self.btc_network,
                    self.config.identifier,
                    Some(signing_session_id.clone()),
                    &SigningRound1Error::AlreadyInSigningSession.to_string(),
                );
            }
            return Err(tonic::Status::internal("Already in signing session"));
        }

        let mut psbt = Psbt::deserialize(req.psbt.as_slice()).to_status()?;

        let nonces = signer::get_round1_signing_package(
            &mut psbt,
            self.min_signers,
            &self.db,
            &self.identifier,
        )
        .await
        .map_err(|e| SigningError::Round1(e))
        .to_status()?;

        // Save signing nonces in memory
        let signing_nonces =
            nonces.iter().map(|nonce| (nonce.0.clone(), nonce.1)).collect::<Vec<_>>();
        nonces_lock.replace(signing_nonces);

        let psbt_bytes = hex::decode(psbt.serialize_hex())
            .map_err(|e| internal!("Failed to serialize psbt: {}", e))?;

        let res = rpc::SigningPackage {
            identifier: self.identifier.serialize().to_vec(),
            psbt: psbt_bytes,
            signing_session_id: signing_session_id.to_vec(),
        };

        Ok(tonic::Response::new(res))
    }

    async fn get_round2_signing_package(
        &self,
        req: tonic::Request<rpc::SigningPackageRequest>,
    ) -> Result<tonic::Response<rpc::SigningPackage>, tonic::Status> {
        self.validate_jwt(&req)?;
        // Ensure we have a key package
        self.db.get_key_package().to_status()?;

        // If we have no nonces, we are not in a signing session
        let mut nonces_lock = self.frost_round1_nonces.lock().await;
        if nonces_lock.is_none() {
            return Err(tonic::Status::internal("Not in signing session"));
        }
        let signing_nonces = nonces_lock.clone().unwrap();

        // Validate PSBT
        let req = req.into_inner();
        info!("Received round2 signing package request");
        let signing_session_id = parse_signing_session_id(&req.signing_session_id).to_status()?;

        let mut psbt = Psbt::deserialize(req.psbt.as_slice()).to_status()?;

        signer::get_round2_signing_package(
            &mut psbt,
            self.min_signers,
            &self.db,
            &self.identifier,
            &signing_nonces,
        )
        .await
        .map_err(|e| {
            if let Some(telemetry) = self.telemetry.as_ref() {
                telemetry.update_signing_error_metrics(
                    self.btc_network,
                    self.config.identifier,
                    Some(signing_session_id.clone()),
                    &e.to_string(),
                );
            }

            SigningError::Round2(e)
        })
        .to_status()?;

        // We are done signing, remove the nonces
        nonces_lock.take();
        drop(nonces_lock);
        let psbt_bytes = hex::decode(psbt.serialize_hex())
            .map_err(|e| internal!("Failed to serialize psbt: {}", e))?;

        let signed_tx = psbt.extract_tx().expect("just checked in get_round2_signing_package");

        // We just signed for all pending pegouts lets start tracking them
        if cfg!(feature = "conflicting_input") {
            let pending_pegouts = self.db.get_pending_pegouts().to_status()?;
            let pending_pegout_ids =
                pending_pegouts.iter().map(|p| p.id).collect::<Vec<PegoutId>>();
            self.add_tracked_tx(signed_tx.clone(), &pending_pegouts, SystemTime::now())
                .await
                .to_status()?;
            self.db.remove_pending_pegout(&pending_pegout_ids).to_status()?;
            self.db.flush().to_status()?;
        }

        let res = rpc::SigningPackage {
            identifier: self.identifier.serialize().to_vec(),
            psbt: psbt_bytes,
            signing_session_id: signing_session_id.to_vec(),
        };

        Ok(tonic::Response::new(res))
    }

    /* Coordinator Endpoints */
    async fn finalize_signing(
        &self,
        req: tonic::Request<rpc::FinalizeSigningRequest>,
    ) -> Result<tonic::Response<rpc::FinalizeSigningResponse>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        info!(
            "Received finalize signing request with signing session id: {:?}",
            hex::encode(req.signing_session_id.clone())
        );
        let signing_session_id =
            parse_signing_session_id(&req.signing_session_id).map_err(|e| {
                error!("Failed to parse signing session id: {}", e);
                badarg!("Failed to parse signing session id: {}", e)
            })?;

        let _tx_lock = self.tx_lock.lock().await;
        let psbt =
            coordinator::finalize_signing(&signing_session_id, &self.db).await.map_err(|e| {
                internal!(
                    "Failed to finalize signing: {}, signing session id: {:?}",
                    e,
                    hex::encode(signing_session_id)
                )
            })?;
        // This should be a ready to broadcast tx
        let tx = psbt.clone().extract_tx().to_status()?;
        let tx_id = match self.bitcoind_client.send_raw_transaction(&tx) {
            Ok(tx_id) => Ok(Some(tx_id)),
            Err(err) => {
                let err_msg = err.to_string();
                if err_msg.contains("already in chain") {
                    Ok(None)
                } else {
                    error!("Failed to broadcast tx: {}", err);
                    Err(CoordinatorError::FailedToBroadcastTx(err))
                }
            }
        }
        .to_status()?;

        // If the coordinator participated in signing they have already tracked the tx
        // however, if the coordinator did not participate in signing they need to track the
        // tx now
        let pending_pegouts = self.db.get_pending_pegouts().to_status()?;
        let pegout_ids = psbt
            .pegout_ids()
            .iter()
            .map(|p| PegoutId::from_bytes(p).expect("values are 36 bytes"))
            .collect::<Vec<PegoutId>>();

        let pending_pegouts =
            pending_pegouts.into_iter().filter(|p| pegout_ids.contains(&p.id)).collect::<Vec<_>>();

        self.add_tracked_tx(tx.clone(), &pending_pegouts, SystemTime::now()).await.to_status()?;
        self.db.remove_pending_pegout(&pegout_ids).to_status()?;
        self.db.flush().to_status()?;

        if let Some(tx_id) = tx_id {
            info!("Broadcasted tx: {:?}", tx_id);
        } else {
            info!("Transaction already broadcasted and in pool");
        }

        let psbt_bytes = hex::decode(psbt.serialize_hex())
            .map_err(|e| internal!("Failed to serialize psbt: {}", e))?;

        let res = tonic::Response::new(rpc::FinalizeSigningResponse { psbt: psbt_bytes });
        Ok(res)
    }

    async fn get_psbt(
        &self,
        req: tonic::Request<rpc::MakeTxRequest>,
    ) -> Result<tonic::Response<rpc::SigningPackage>, tonic::Status> {
        self.validate_jwt(&req)?;
        // Ensure we have a key package
        self.db.get_key_package().to_status()?;
        // take a lock on the tx_lock
        let _tx_lock = self.tx_lock.lock();

        let req = req.into_inner();
        info!(
            "Received make tx request for signing session id: {:?}",
            hex::encode(req.signing_session_id.clone())
        );
        let signing_session_id = parse_signing_session_id(&req.signing_session_id).to_status()?;
        let checkpoint = BlockHash::from_slice(&req.checkpoint_block_hash)
            .map_err(|e| badarg!("invalid checkpoint hash: {}", e))?;

        let fee_res = self
            .bitcoind_client
            .estimate_smart_fee(1, Some(bitcoincore_rpc::json::EstimateMode::Conservative));
        let mut fee_rate = self.fall_back_fee_rate;
        if let Ok(fee) = fee_res {
            if let Some(f) = fee.fee_rate {
                fee_rate = btc_per_kb_to_sat_per_vb(f);
            }
        }

        debug!("Cord Fee rate: {:?}", fee_rate);
        let pending_pegouts = self.db.get_pending_pegouts().to_status()?;
        if let Some(telemetry) = self.telemetry.as_ref() {
            telemetry.update_pending_pegouts(pending_pegouts.len() as i64);
        }
        let outputs = pending_pegouts
            .iter()
            .map(|p| (TxOut { value: p.value, script_pubkey: p.spk.clone() }, p.id))
            .collect::<Vec<(TxOut, PegoutId)>>();

        let pk_package = self
            .db
            .get_key_package()
            .to_status()?
            .ok_or_else(|| internal!("missing key package, run the dkg process first"))?;

        let secp_pk = pk_package
            .verifying_key()
            .to_secp_pk()
            .map_err(|e| internal!("Failed to generate tweaked public key: {}", e))?;
        let change_script = wallet::address::generate_taproot_change_scriptpubkey(&secp_pk);

        // First sync the pegout scheduler
        self.sync_pegout_scheduler(checkpoint).await.to_status()?;
        let tracked_txs = self.db.get_tracked_txs().to_status()?;

        let psbt = coordinator::make_tx(
            outputs,
            fee_rate,
            change_script,
            &self.db,
            self.min_signers,
            tracked_txs,
        )
        .await
        .to_status()?;

        // Save psbt to db
        self.db.update_psbt(&signing_session_id, &psbt).to_status()?;
        self.db.flush().to_status()?;

        let psbt_bytes = hex::decode(psbt.serialize_hex()).to_status()?;
        let res = tonic::Response::new(rpc::SigningPackage {
            // identifier really doent matter here.
            identifier: self.identifier.serialize().to_vec(),
            psbt: psbt_bytes,
            signing_session_id: signing_session_id.to_vec(),
        });
        Ok(res)
    }

    async fn get_to_sign_package(
        &self,
        req: tonic::Request<rpc::ToSignRequest>,
    ) -> Result<tonic::Response<rpc::SigningPackage>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();

        info!(
            "Received to sign package request, signing session id: {:?}",
            hex::encode(req.signing_session_id.clone())
        );
        let signing_session_id = parse_signing_session_id(&req.signing_session_id).to_status()?;
        let psbt = coordinator::get_to_sign(&signing_session_id, &self.db, self.min_signers)
            .to_status()?;

        let psbt_bytes = hex::decode(psbt.serialize_hex())
            .map_err(|e| internal!("Failed to serialize psbt: {}", e))?;
        let res = tonic::Response::new(rpc::SigningPackage {
            // identifier really doent matter here.
            identifier: self.identifier.serialize().to_vec(),
            psbt: psbt_bytes,
            signing_session_id: signing_session_id.to_vec(),
        });
        Ok(res)
    }

    async fn new_round1_signing_package(
        &self,
        req: tonic::Request<rpc::SigningPackage>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&req)?;
        // Ensure we have a key package
        self.db.get_key_package().to_status()?;

        let req = req.into_inner();
        info!(
            "Received new round1 signing package for signing session id: {:?}",
            hex::encode(req.signing_session_id.clone())
        );
        let signing_session_id = parse_signing_session_id(&req.signing_session_id).to_status()?;
        let frost_id = deserialize_frost_peer_id(req.identifier).to_status()?;
        let psbt = Psbt::deserialize(req.psbt.as_slice()).to_status()?;

        coordinator::add_round1_signing(
            &signing_session_id,
            frost_id,
            &psbt,
            &self.db,
            self.min_signers,
        )
        .to_status()?;

        // if let Some(telemetry) = self.telemetry.as_ref() {
        //     telemetry.update_round1_signing_metrics(
        //         self.btc_network,
        //         self.config.identifier,
        //         &signing_session_id,
        //         written_data,
        //         start.elapsed().as_millis(),
        //     )
        // }

        Ok(tonic::Response::new(rpc::Empty {}))
    }

    async fn new_round2_signing_package(
        &self,
        req: tonic::Request<rpc::SigningPackage>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&req)?;
        // Ensure we have a key package
        self.db.get_key_package().to_status()?;

        let req = req.into_inner();
        info!("Received round2 signing package");
        let signing_session_id = parse_signing_session_id(&req.signing_session_id).to_status()?;
        let frost_id = deserialize_frost_peer_id(req.identifier).to_status()?;
        let psbt = Psbt::deserialize(req.psbt.as_slice()).to_status()?;

        coordinator::add_round2_signing(
            &signing_session_id,
            frost_id,
            &psbt,
            &self.db,
            self.min_signers,
        )
        .to_status()?;

        Ok(tonic::Response::new(rpc::Empty {}))
    }

    /* Address Endpoints */

    async fn get_public_key(
        &self,
        req: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::GetPublicKeyResponse>, tonic::Status> {
        self.validate_jwt(&req)?;
        // Ensure we have a key package
        let key_package =
            self.db.get_key_package().to_status()?.ok_or(badarg!("Missing key package"))?;

        let pk = key_package.verifying_key();
        let pk = hex::encode(pk.serialize().to_status()?);

        return Ok(tonic::Response::new(rpc::GetPublicKeyResponse { publickey: pk }));
    }

    async fn get_gateway_address(
        &self,
        req: tonic::Request<rpc::GetGatewayAddressRequest>,
    ) -> Result<tonic::Response<rpc::GetGatewayAddressResponse>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        // Ensure we have a key package
        let key_package = self
            .db
            .get_key_package()
            .to_status()?
            .ok_or(tonic::Status::internal("Missing key package"))?;

        let eth_address = parse_eth_address(req.eth_address).to_status()?;
        let agg_key = key_package.verifying_key();
        let tweaked_key = generate_tweaked_public_key(agg_key, &eth_address)
            .map_err(|e| internal!("Failed to generate tweaked public key: {}", e))?;
        let gateway_address = generate_taproot_address(&tweaked_key, self.btc_network);

        return Ok(tonic::Response::new(rpc::GetGatewayAddressResponse {
            publickey: hex::encode(
                agg_key
                    .serialize()
                    .map_err(|e| internal!("Failed to serialize public key: {}", e))?,
            ),
            tweaked_public_key: hex::encode(tweaked_key.serialize()),
            gateway_address: gateway_address.to_string(),
        }));
    }

    /* DKG Endpoints */
    /// Adds round 1 pkg received from another peer to our own state
    async fn new_round1_dkg_package(
        &self,
        req: tonic::Request<rpc::DkgPayload>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&req)?;
        // If there is already a key package, we don't need to add round 1 dkg
        if self.db.get_key_package().to_status()?.is_some() {
            return Err(already_exists!("already have key package"));
        }

        let req = req.into_inner();
        let frost_id = deserialize_frost_peer_id(req.identifier.clone()).to_status()?;
        let dkg_round1 =
            frost::keys::dkg::round1::Package::deserialize(req.payload.as_slice()).to_status()?;

        if frost_id == self.identifier {
            return Err(badarg!("Cannot add own round1 dkg package"));
        }

        if self
            .frost_round1_dkg
            .lock()
            .await
            .as_ref()
            .ok_or(DKGError::MissingRound1DkgPayload)
            .to_status()?
            .1 ==
            dkg_round1
        {
            return Err(badarg!("Cannot add own round1 dkg package"));
        }
        // Should not add if we have max signers
        if self.db.get_round1_dkg_packages().to_status()?.len() as u16 == self.max_signers - 1 {
            return Err(badarg!("dkg max signers reached"));
        }

        self.db.add_round1_dkg(frost_id, dkg_round1).to_status()?;
        self.db.flush().to_status()?;

        Ok(tonic::Response::new(rpc::Empty {}))
    }

    /// Gets round 1 pkg we have generated (to be sent to another peer) - default when we start the
    /// btc server
    async fn get_round1_dkg_package(
        &self,
        req: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::DkgPayload>, tonic::Status> {
        self.validate_jwt(&req)?;
        // If there is already a key package, we don't need to add round 1 dkg
        if self.db.get_key_package().to_status()?.is_some() {
            return Err(already_exists!("already have key package"));
        }

        let round1_dkg = self
            .frost_round1_dkg
            .lock()
            .await
            .clone()
            .ok_or(badarg!("Missing round1 dkg package"))?
            .1;

        let res = rpc::DkgPayload {
            identifier: self.identifier.serialize().to_vec(),
            payload: round1_dkg
                .serialize()
                .map_err(|e| internal!("Failed to serialize round1 dkg package: {}", e))?
                .to_vec(),
        };

        Ok(tonic::Response::new(res))
    }

    /// Gets round 1 pkgs we have collected so far - includes our own package
    async fn get_round1_dkg_packages(
        &self,
        req: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::DkgPayload>, tonic::Status> {
        self.validate_jwt(&req)?;
        if self.db.get_public_key_package().to_status()?.is_some() {
            warn!("receivednotification about round 2 DKG while having key package");
            return Err(already_exists!("already have key package"));
        }

        let round1_packages = self.db.get_round1_dkg_packages().to_status()?;

        let json = serde_json::to_string(&round1_packages)
            .map_err(|e| internal!("Failed to serialize round1 dkg packages: {}", e))?;
        let res = rpc::DkgPayload {
            identifier: self.identifier.serialize().to_vec(),
            payload: json.as_bytes().to_vec(),
        };
        Ok(tonic::Response::new(res))
    }

    /// Generates a hashmap of round2 packages for sending to all other peers (needs round 1
    /// packages)
    async fn get_round2_dkg_package(
        &self,
        req: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::DkgPayload>, tonic::Status> {
        self.validate_jwt(&req)?;
        // If there is already a key package, we don't need to add round 2 dkg
        if self.db.get_key_package().to_status()?.is_some() {
            return Err(already_exists!("already have key package"));
        }

        match self.frost_round1_dkg.lock().await.clone() {
            Some(round1_dkg) => {
                // Retrieve round 1 packages from peers
                // Here we don't check we have enough that should be done by the frost lib
                // So we just propagate the error
                let round1_packages = self.db.get_round1_dkg_packages().to_status()?;

                let (round2_secret_package, round2_packages) =
                    frost::keys::dkg::part2(round1_dkg.0.clone(), &round1_packages).to_status()?;
                self.frost_round2_dkg.lock().await.replace(round2_secret_package.clone());

                // Each package is unique for a peer.
                // Upstream caller must ensure that the package is sent to that specific peer
                // let round2_packages = self.db.get_round2_dkg_packages().to_status()?;
                let json = serde_json::to_string(&round2_packages)
                    .map_err(|e| internal!("Failed to serialize round2 dkg packages: {}", e))?;
                let res = rpc::DkgPayload {
                    identifier: self.identifier.serialize().to_vec(),
                    payload: json.as_bytes().to_vec(),
                };
                Ok(tonic::Response::new(res))
            }
            None => {
                return Err(badarg!("not enough round1 packages"));
            }
        }

        // Each package is unique for a peer.
    }

    async fn new_round2_dkg_package(
        &self,
        req: tonic::Request<rpc::DkgPayload>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&req)?;
        // If there is already a key package, we don't need to add round 2 dkg
        if self.db.get_key_package().to_status()?.is_some() {
            return Err(already_exists!("already have key package"));
        }

        let req = req.into_inner();
        let frost_id = deserialize_frost_peer_id(req.identifier).to_status()?;
        let package: frost::keys::dkg::round2::Package =
            serde_json::from_slice(req.payload.as_slice())
                .map_err(|e| internal!("Failed to deserialize round2 dkg package: {}", e))?;

        if frost_id == self.identifier {
            return Err(badarg!("cannot add own dkg package"));
        }

        self.db.add_round2_dkg(frost_id, package).to_status()?;

        // If we have a max_signers round2 packages we can generate and save the key package
        let mut dkg_done = false;
        let round2_packages = self.db.get_round2_dkg_packages().to_status()?;
        if round2_packages.len() as u16 == self.max_signers - 1 {
            let round1_packages = self.db.get_round1_dkg_packages().to_status()?;
            if let Some(round2_secret) = self.frost_round2_dkg.lock().await.clone() {
                let pk_res =
                    frost::keys::dkg::part3(&round2_secret, &round1_packages, &round2_packages)
                        .to_status()?;

                self.db.set_key_package(pk_res.0.clone()).to_status()?;
                self.db.set_pubkey_package(pk_res.1.clone()).to_status()?;
                self.db.flush().to_status()?;

                dkg_done = true;
            } else {
                return Err(badarg!("invalid round2 dkg payload missing package"));
            }
        }

        // Signing at this point is successful
        // Clear out round 2 secret
        if dkg_done {
            self.frost_round2_dkg.lock().await.take();
            self.frost_round1_dkg.lock().await.take();
        }

        // if let Some(telemetry) = self.telemetry.as_ref() {
        //     telemetry.update_round2_dkg_metrics(
        //         self.btc_network,
        //         self.config.identifier,
        //         data_written,
        //         start.elapsed().as_millis(),
        //     )
        // }

        Ok(tonic::Response::new(rpc::Empty {}))
    }

    // Currently not used
    async fn signer_finalize(
        &self,
        _req: tonic::Request<rpc::FinalizeSignerRequest>,
    ) -> Result<tonic::Response<rpc::FinalizeSigningResponse>, tonic::Status> {
        panic!("Not used");
        // let req = req.into_inner();
        // info!("Received finalize signer request");

        // let finalized_psbt = bitcoin::Psbt::deserialize(&req.psbt).map_err(|e| {
        //     error!("Failed to deserialize psbt: {}", e);
        //     badarg!("Failed to deserialize psbt: {}", e)
        // })?;

        // let psbt = self
        //     .finalize_signer(finalized_psbt)
        //     .await
        //     .map_err(|e| internal!("Failed to finalize signer: {}", e))?;
        // let psbt_bytes = hex::decode(psbt.serialize_hex())
        //     .map_err(|e| internal!("Failed to serialize psbt: {}", e))?;

        // let res = tonic::Response::new(rpc::FinalizeSigningResponse {
        //     psbt: bitcoin::consensus::encode::serialize(&psbt_bytes),
        // });

        // if let Some(telemetry) = self.telemetry.as_ref() {
        //     telemetry.record_finalized_signing_sessions(self.btc_network, self.config.identifier)
        // }

        // Ok(res)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<(), Box<dyn std::error::Error>> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .filter_module("btc_server::", log::LevelFilter::Trace)
        .filter_module("bitcoincore_rpc::", log::LevelFilter::Trace)
        .init();

    let config = btcserverlib::config::load_config()?;

    let telemetry = if config.metrics_port.is_some() {
        let telemetry = Telemetry::new().await?;
        telemetry.start().await?;
        Some(telemetry)
    } else {
        None
    };

    // setup the grpc server
    let bitcoind_client = bitcoincore_rpc::Client::new(
        config.bitcoind_url.as_str(),
        Auth::UserPass(config.bitcoind_user.clone(), config.bitcoind_pass.clone()),
    )
    .expect("bitcoind client");
    let btc_server: App<bitcoincore_rpc::Client> =
        App::new(config.clone(), bitcoind_client, telemetry.clone())?;

    // run grpc server in the background
    let grpc_stop_tx = match btc_server.serve_async().await {
        Ok(s) => {
            info!("Grpc server: started successfully on {:?}", config.address);
            info!("Grpc server: waiting for a shutdown signal...");
            Some(s)
        }
        Err(err) => {
            error!("Grpc server: Join Error {}", err.to_string());
            None
        }
    };

    // spawn terminate handlers routine
    let grpc_join_handle = tokio::spawn(stop_signal(grpc_stop_tx));

    let server_handle = if let Some(telemetry) = telemetry {
        // create and spin up the http server
        let state = ServerState::new(telemetry.clone()).await;
        // create the actix webserver
        let port = config.metrics_port.unwrap_or(7000);
        let grpc_server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port);
        let grpc_server = create_web_server(state, grpc_server_addr)?;
        // get server handle
        let server_handle = grpc_server.handle();
        // spawn the server in the background
        tokio::spawn(async move {
            if let Err(err) = grpc_server.await {
                error!("Actix Web server error: {:?}", err);
            }
        });
        info!("Grpc server started.");
        Some(server_handle)
    } else {
        info!("Telemetry is disabled. Not starting the http server.");
        None
    };

    // // block and wait for a shutdown signal to terminate
    let _ = tokio::join!(grpc_join_handle);

    info!("Grpc server stopped");

    if let Some(server_handle) = server_handle {
        // Await the Actix server shutdown
        info!("Stopping actix server ...");
        server_handle.stop(true).await;
        info!("Actix server stopped. Goodbye!");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, str::FromStr};

    use bitcoin::{OutPoint, Script, Txid};
    use rand::Rng;
    use tempfile::TempDir;
    use url::Url;

    use super::*;
    use btcserverlib::{
        frost_id,
        test_utils::{
            create_random_pegout_id, create_tx, random_p2wpkh_script, trusted_dealer_setup,
            MockBitcoind,
        },
    };

    async fn setup() -> App<MockBitcoind> {
        let temp_dir = TempDir::new().unwrap();
        let bitcoind_client = MockBitcoind::new();
        let config = Config {
            db: temp_dir.into_path(),
            btc_network: bitcoin::Network::Regtest,
            identifier: 0,
            address: "0.0.0.0:8080".to_string(),
            max_signers: 3,
            min_signers: 2,
            toml: None,
            btc_signing_server_jwt_secret: None,
            bitcoind_url: Url::from_str("http://localhost:8332").unwrap(),
            bitcoind_user: "user".to_string(),
            bitcoind_pass: "pass".to_string(),
            metrics_port: Some(8080),
            fee_rate_diff_percentage: 10,
            fall_back_fee_rate_sat_per_vbyte: 1000,
        };

        let app = App::new(config, bitcoind_client, None).expect("btc server");

        app
    }

    #[tokio::test]
    async fn should_not_get_public_key_without_dkg() {
        let app = setup().await;
        let req = tonic::Request::new(rpc::Empty {});
        let res = app.get_public_key(req).await.unwrap_err();
        assert_eq!(res.code(), tonic::Code::InvalidArgument);
        assert_eq!(res.message(), "Missing key package");
    }

    #[tokio::test]
    async fn round1_dkg_should_work_if_missing_key_package() {
        let app = setup().await;
        let req = tonic::Request::new(rpc::Empty {});
        let round1_dkg = app.get_round1_dkg_package(req).await.unwrap();
        let inner = round1_dkg.into_inner();
        let frost_id = deserialize_frost_peer_id(inner.identifier).unwrap();
        assert_eq!(frost_id, frost_id!(0));
        let payload = inner.payload;
        let _round1_dkg_pkg = frost::keys::dkg::round1::Package::deserialize(&payload).unwrap();
        // Not much to assert on here, just that we can deserialize the package
    }

    #[tokio::test]
    async fn round1_dkg_should_fail_if_key_package_already_exists() {
        let app = setup().await;
        let (secret_shares, pk_pkg) = trusted_dealer_setup(2, 3);
        let secret_share = secret_shares.values().collect::<Vec<_>>()[0];
        // derive key package for some participant
        let key_package =
            frost::keys::KeyPackage::try_from(secret_share.clone()).expect("valid key package");

        app.db.set_pubkey_package(pk_pkg.clone()).unwrap();
        app.db.set_key_package(key_package.clone()).unwrap();

        let req = tonic::Request::new(rpc::Empty {});
        let res = app.get_round1_dkg_package(req).await.unwrap_err();
        assert_eq!(res.code(), tonic::Code::AlreadyExists);
        assert_eq!(res.message(), "already have key package");
    }

    #[tokio::test]
    async fn round1_dkg_should_get_round1_dkg() {
        let rng = thread_rng();
        let mut app = setup().await;
        // Save round 1 secret package
        app.frost_round1_dkg = Arc::new(Mutex::new(Some(
            frost::keys::dkg::part1(app.identifier, app.max_signers, app.min_signers, rng.clone())
                .unwrap(),
        )));

        // can get round 1 dkg
        let req = tonic::Request::new(rpc::Empty {});
        let round1_dkg = app.get_round1_dkg_package(req).await.unwrap();
        let inner_1 = round1_dkg.into_inner();

        // if we repeat the call we should get the same result
        let req = tonic::Request::new(rpc::Empty {});
        let round1_dkg2 = app.get_round1_dkg_package(req).await.unwrap();
        let inner_2 = round1_dkg2.into_inner();
        assert_eq!(inner_1, inner_2);

        // However if we modify the round1_dkg we should get a different result
        // we don't have to modify the whole package, the rng should create new coefficients
        app.frost_round1_dkg = Arc::new(Mutex::new(Some(
            frost::keys::dkg::part1(app.identifier, app.max_signers, app.min_signers, rng.clone())
                .unwrap(),
        )));

        let req = tonic::Request::new(rpc::Empty {});
        let round1_dkg3 = app.get_round1_dkg_package(req).await.unwrap();
        let inner_3 = round1_dkg3.into_inner();
        assert_ne!(inner_1, inner_3);
    }

    #[tokio::test]
    async fn should_add_round1_dkg() {
        let mut app = setup().await;
        let rng = thread_rng();
        let mut round1_dkgs = vec![];
        for index in 1..(app.max_signers + 1) {
            round1_dkgs.push(
                frost::keys::dkg::part1(
                    frost_id!(index),
                    app.max_signers,
                    app.min_signers,
                    rng.clone(),
                )
                .unwrap(),
            );
        }
        app.frost_round1_dkg = Arc::new(Mutex::new(Some(round1_dkgs[0].clone())));

        // Should not be able to add ourselves -- when identifier is the same
        let req = tonic::Request::new(rpc::DkgPayload {
            identifier: app.identifier.serialize().to_vec(),
            payload: round1_dkgs[0].clone().1.serialize().unwrap().to_vec(),
        });
        let res = app.new_round1_dkg_package(req).await.unwrap_err();
        assert_eq!(res.code(), tonic::Code::InvalidArgument);
        assert_eq!(res.message(), "Cannot add own round1 dkg package");

        // Should not be able to add ourselves -- when package is the same but frost id is the same
        let req = tonic::Request::new(rpc::DkgPayload {
            identifier: frost_id!(2).serialize().to_vec(),
            payload: round1_dkgs[0].clone().1.serialize().unwrap().to_vec(),
        });
        let res = app.new_round1_dkg_package(req).await.unwrap_err();
        assert_eq!(res.code(), tonic::Code::InvalidArgument);
        assert_eq!(res.message(), "Cannot add own round1 dkg package");

        // Add round 1 dkg from peer
        let req = tonic::Request::new(rpc::DkgPayload {
            identifier: frost_id!(2).serialize().to_vec(),
            payload: round1_dkgs[1].clone().1.serialize().unwrap().to_vec(),
        });
        app.new_round1_dkg_package(req).await.unwrap();

        // Check it updates the db
        let req = tonic::Request::new(rpc::Empty {});
        let pkgs = app.get_round1_dkg_packages(req).await.unwrap();
        let inner = pkgs.into_inner();
        let pkgs: HashMap<frost::Identifier, frost::keys::dkg::round1::Package> =
            serde_json::from_slice(&inner.payload).unwrap();
        assert!(pkgs.contains_key(&frost_id!(2)));
        assert_eq!(pkgs.len(), 1);

        // Adding the same round 1 package should not make a difference
        let req = tonic::Request::new(rpc::Empty {});
        let pkgs = app.get_round1_dkg_packages(req).await.unwrap();
        let inner = pkgs.into_inner();
        let pkgs: HashMap<frost::Identifier, frost::keys::dkg::round1::Package> =
            serde_json::from_slice(&inner.payload).unwrap();
        assert!(pkgs.contains_key(&frost_id!(2)));
        assert_eq!(pkgs.len(), 1);

        // Should be able to add different round 1 dkg
        let req = tonic::Request::new(rpc::DkgPayload {
            identifier: frost_id!(3).serialize().to_vec(),
            payload: round1_dkgs[2].clone().1.serialize().unwrap().to_vec(),
        });
        app.new_round1_dkg_package(req).await.unwrap();

        let req = tonic::Request::new(rpc::Empty {});
        let pkgs = app.get_round1_dkg_packages(req).await.unwrap();
        let inner = pkgs.into_inner();
        let pkgs: HashMap<frost::Identifier, frost::keys::dkg::round1::Package> =
            serde_json::from_slice(&inner.payload).unwrap();
        assert!(pkgs.contains_key(&frost_id!(2)));
        assert!(pkgs.contains_key(&frost_id!(3)));
        assert_eq!(pkgs.len(), 2);

        // Try to add one more participant
        let extra =
            frost::keys::dkg::part1(frost_id!(4), app.max_signers, app.min_signers, rng.clone())
                .unwrap();

        let req = tonic::Request::new(rpc::DkgPayload {
            identifier: frost_id!(4).serialize().to_vec(),
            payload: extra.1.serialize().unwrap().to_vec(),
        });
        let res = app.new_round1_dkg_package(req).await.unwrap_err();
        assert_eq!(res.code(), tonic::Code::InvalidArgument);
        assert_eq!(res.message(), "dkg max signers reached");
    }

    #[tokio::test]
    async fn should_not_get_round2_dkg_when_keys_exist() {
        let app = setup().await;
        let (secret_shares, pk_pkg) = trusted_dealer_setup(2, 3);
        let secret_share = secret_shares.values().collect::<Vec<_>>()[0];
        // derive key package for some participant
        let key_package =
            frost::keys::KeyPackage::try_from(secret_share.clone()).expect("valid key package");

        app.db.set_pubkey_package(pk_pkg.clone()).unwrap();
        app.db.set_key_package(key_package.clone()).unwrap();

        let req = tonic::Request::new(rpc::Empty {});
        let res = app.get_round2_dkg_package(req).await.unwrap_err();
        assert_eq!(res.code(), tonic::Code::AlreadyExists);
        assert_eq!(res.message(), "already have key package");
    }

    #[tokio::test]
    async fn round2_dkg_fails_when_missing_round1_secret() {
        let app = setup().await;
        let req = tonic::Request::new(rpc::Empty {});
        let res = app.get_round2_dkg_package(req).await.unwrap_err();
        assert_eq!(res.code(), tonic::Code::Internal);
        assert_eq!(res.message(), "internal error: Frost error: Incorrect number of packages.");
    }

    #[tokio::test]
    async fn round2_dkg_get_packages() {
        let mut app = setup().await;
        let rng = thread_rng();
        let mut round1_dkgs = vec![];
        // reminder that frost identifiers start at 1
        for index in 0..(app.max_signers) {
            round1_dkgs.push(
                frost::keys::dkg::part1(
                    frost_id!(index),
                    app.max_signers,
                    app.min_signers,
                    rng.clone(),
                )
                .unwrap(),
            );
        }
        app.frost_round1_dkg = Arc::new(Mutex::new(Some(round1_dkgs[0].clone())));

        let req = tonic::Request::new(rpc::Empty {});
        let res = app.get_round2_dkg_package(req).await.unwrap_err();
        assert_eq!(res.code(), tonic::Code::Internal);
        assert_eq!(res.message(), "internal error: Frost error: Incorrect number of packages.");

        // Lets add the round 1 dkg for the first two participants
        let req = tonic::Request::new(rpc::DkgPayload {
            identifier: frost_id!(1).serialize().to_vec(),
            payload: round1_dkgs[1].clone().1.serialize().unwrap().to_vec(),
        });
        app.new_round1_dkg_package(req).await.unwrap();

        let req = tonic::Request::new(rpc::DkgPayload {
            identifier: frost_id!(2).serialize().to_vec(),
            payload: round1_dkgs[2].clone().1.serialize().unwrap().to_vec(),
        });
        app.new_round1_dkg_package(req).await.unwrap();

        // Now we should be able to get the round 2 dkg
        let req = tonic::Request::new(rpc::Empty {});
        let res = app.get_round2_dkg_package(req).await.unwrap();
        let inner = res.into_inner();
        let pkgs: HashMap<frost::Identifier, frost::keys::dkg::round2::Package> =
            serde_json::from_slice(&inner.payload).unwrap();
        assert!(pkgs.contains_key(&frost_id!(1)));
        assert!(pkgs.contains_key(&frost_id!(2)));
        assert_eq!(pkgs.len(), 2);

        // Ensure the round2 dkg secret package is stored
        assert!(app.frost_round2_dkg.lock().await.is_some());
    }

    #[tokio::test]
    async fn should_not_accept_round2_dkg_from_ourselves() {
        let app = setup().await;
        let rng = thread_rng();
        let mut round1_dkgs = vec![];
        // reminder that frost identifiers start at 1
        for index in 0..(app.max_signers) {
            round1_dkgs.push(
                frost::keys::dkg::part1(
                    frost_id!(index),
                    app.max_signers,
                    app.min_signers,
                    rng.clone(),
                )
                .unwrap(),
            );
        }

        // Add round 1 dkg for the first two participants
        let req = tonic::Request::new(rpc::DkgPayload {
            identifier: frost_id!(1).serialize().to_vec(),
            payload: round1_dkgs[1].clone().1.serialize().unwrap().to_vec(),
        });
        app.new_round1_dkg_package(req).await.unwrap();
        let req = tonic::Request::new(rpc::DkgPayload {
            identifier: frost_id!(2).serialize().to_vec(),
            payload: round1_dkgs[2].clone().1.serialize().unwrap().to_vec(),
        });
        app.new_round1_dkg_package(req).await.unwrap();

        let req = tonic::Request::new(rpc::Empty {});
        let round2_dkg = app.get_round2_dkg_package(req).await.unwrap();
        let inner = round2_dkg.into_inner();
        let pkgs: HashMap<frost::Identifier, frost::keys::dkg::round2::Package> =
            serde_json::from_slice(&inner.payload).unwrap();
        assert!(pkgs.contains_key(&frost_id!(1)));
        assert!(pkgs.contains_key(&frost_id!(2)));
        assert_eq!(pkgs.len(), 2);

        // Try to add round 2 dkg from ourselves
        let req = tonic::Request::new(rpc::DkgPayload {
            identifier: frost_id!(0).serialize().to_vec(),
            payload: serde_json::to_vec(&pkgs.get(&frost_id!(1))).unwrap(),
        });
        let res = app.new_round2_dkg_package(req).await.unwrap_err();
        assert_eq!(res.code(), tonic::Code::InvalidArgument);
        assert_eq!(res.message(), "cannot add own dkg package");
    }

    #[tokio::test]
    async fn get_all_utxos() {
        let app = setup().await;
        let req = tonic::Request::new(rpc::Empty {});
        let res = app.get_all_utxos(req).await.unwrap();
        let inner = res.into_inner();
        assert!(inner.utxos.is_empty());
    }

    #[tokio::test]
    async fn get_all_utxos_with_data() {
        let app = setup().await;
        let mut rng = thread_rng();

        for _ in 0..100 {
            let txid = Txid::from_slice(&rng.gen::<[u8; 32]>()).unwrap();
            let vout = rng.gen_range(0..u32::MAX);
            let value = rng.gen_range(1..1_000_000);
            let script_bytes: Vec<u8> = (0..20).map(|_| rng.gen()).collect();
            let script = Script::from_bytes(script_bytes.as_slice());

            let utxo = crate::database::Utxo::new(
                OutPoint::new(txid, vout),
                TxOut { value: Amount::from_sat(value), script_pubkey: script.into() },
                None,
                None,
            );
            app.db.store_utxos(&[&utxo]).expect("Failed to store UTXO");
        }

        let req = tonic::Request::new(rpc::Empty {});
        let res = app.get_all_utxos(req).await.unwrap();
        let inner = res.into_inner();
        assert!(!inner.utxos.is_empty());
        assert_eq!(inner.utxos.len(), 100);
    }

    #[tokio::test]
    async fn new_consensus_checkpoint() {
        let app = setup().await;
        let (shares, pk_package) = trusted_dealer_setup(app.min_signers, app.max_signers);
        let key_package = frost::keys::KeyPackage::try_from(shares[&app.identifier].clone())
            .expect("valid key package");

        // Add the key packages
        app.db.set_pubkey_package(pk_package.clone()).expect("set public key package");
        app.db.set_key_package(key_package.clone()).expect("set key package");

        // Add some pegin utxos
        let mut pegins = vec![];
        for _ in 0..10 {
            let dummy_tx = create_tx(1, 1, None);
            let utxo = crate::database::Utxo::new(
                dummy_tx.input[0].previous_output,
                dummy_tx.output[0].clone(),
                None,
                None,
            );

            // create pegins btc client can send
            let tx_out = dummy_tx.output.get(utxo.outpoint.vout as usize).expect("valid vout");
            let serialized_script_pub_key = bitcoin::consensus::serialize(&tx_out.script_pubkey);
            let utxo = Utxo {
                outpoint: Some(rpc::OutPoint {
                    txid: bitcoin::consensus::serialize(&utxo.outpoint.txid),
                    vout: utxo.outpoint.vout,
                }),
                output: Some(rpc::TxOut {
                    script_pubkey: Some(rpc::ScriptBuf { script: serialized_script_pub_key }),
                    value: tx_out.value.to_sat(),
                }),
                eth_address: hex::encode(&[0; 20]),
            };
            pegins.push(utxo);
        }

        // Lets add multiple pegouts
        let mut pending_pegouts = vec![];
        for _ in 0..10 {
            let pegout_id = create_random_pegout_id();
            let spk = random_p2wpkh_script();
            pending_pegouts.push(rpc::PendingPegout {
                pegout_id: pegout_id.as_bytes().to_vec(),
                spk: spk.clone().as_bytes().to_vec(),
                amount: 100_000, // sats
                height: 1,
            });
        }

        let req = tonic::Request::new(rpc::ConsensusCheckpointRequest {
            checkpoint_block_hash: BlockHash::all_zeros().to_byte_array().to_vec(),
            pegins: pegins.clone(),
            pending_pegouts: pending_pegouts.clone(),
        });
        let _res = app.new_consensus_checkpoint(req).await.unwrap();

        let pegins_res = app.db.get_all_utxos().expect("valid utxos");
        assert!(pegins.len() == 10);
        for pegin in pegins_res {
            // find by txid
            let original_pegin = pegins
                .iter()
                .find(|p| {
                    let txid = Txid::from_slice(&p.outpoint.as_ref().unwrap().txid).unwrap();
                    txid == pegin.outpoint.txid
                })
                .unwrap();
            assert_eq!(pegin.output.value.to_sat(), original_pegin.output.clone().unwrap().value);
            // TODO(Scott): check script_pubkey
        }

        let pending_pegouts_res = app.db.get_pending_pegouts().expect("valid pending pegouts");
        assert_eq!(pending_pegouts_res.len(), 10);
        for pending_pegout in pending_pegouts_res {
            let original_pegout = pending_pegouts
                .iter()
                .find(|p| p.pegout_id == pending_pegout.id.as_bytes().to_vec())
                .unwrap();
            assert_eq!(pending_pegout.spk.as_bytes().to_vec(), original_pegout.spk);
            assert_eq!(pending_pegout.value.to_sat(), original_pegout.amount);
            assert_eq!(pending_pegout.botanix_height, original_pegout.height);
        }
    }
}
