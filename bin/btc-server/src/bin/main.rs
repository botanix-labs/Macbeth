#[macro_use]
extern crate log;

use base64::{engine::general_purpose, Engine};
use bitcoin::{
    consensus::Decodable, secp256k1, Amount, BlockHash, Psbt, ScriptBuf, Transaction, TxOut,
};
use bitcoin_hashes::Hash;
use bitcoincore_rpc::{Auth, RpcApi};
use btc_server::btc_server_server::{BtcServer, BtcServerServer};
use btcserverlib::{
    badarg,
    config::{Config, Error as ConfigError, GrpcConfig, TomlConfig},
    coordinator::{self, error::CoordinatorError},
    database, dkg,
    federation_args::FederationTomlConfig,
    frost_id, handle_signing_error,
    http::{create_web_server, state::ServerState},
    jwt::{JwtError, JwtSecret},
    measure_rpc_latency,
    merkle::get_wallet_state_commitment,
    pegout_id::PegoutId,
    pegout_scheduler::{self, is_syncing, PegoutRequest, PegoutScheduler},
    rpc::{self, *},
    shutdown::{stop_signal, StopHandle},
    signer::{
        self,
        error::{SigningError, SigningRound1Error, SigningRound2Error},
    },
    telemetry::Telemetry,
    util::{
        btc_per_kb_to_sat_per_vb, deserialize_frost_peer_id, get_available_utxos,
        get_pegin_confirmation_depth, parse_eth_address, parse_signing_session_id, retry_exec,
        ParsingError, UPPER_PEGOUT_BOUND,
    },
    wallet::{
        self,
        address::{generate_taproot_address, generate_tweaked_public_key},
        psbt::{PsbtExt, PsbtOutputExt},
        util::VerifyingKeyExt,
    },
};
use file_descriptor::FILE_DESCRIPTOR_SET;
use frost_secp256k1_tr as frost;
use futures::{pin_mut, StreamExt};
use futures_util::future::FutureExt;
use std::{
    collections::{BTreeMap, HashSet},
    fmt::Debug,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use thiserror::Error;
use tokio::sync::{oneshot, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{codegen::CompressionEncoding, metadata::BinaryMetadataKey, transport::Server};

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
    #[error("DKG state machine error: {0}")]
    DkgStateMachine(#[from] dkg::Error),
    #[error("Dkg deserialization error: {0}")]
    DkgDeserialization(#[from] ciborium::de::Error<std::io::Error>),
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
                Error::DkgStateMachine(dkg) => Err(internal!("Dkg state machine error: {}", dkg)),
                Error::DkgDeserialization(de) => {
                    Err(internal!("Dkg deserialization error: {}", de))
                }
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

/// Print logs about the DKG state machine, for informal or debugging purposes.
fn print_dkg_state_log(dkg: &mut DkgState) {
    // Check session changes.
    if let Some(current) = dkg.machine.session_nonce() {
        if let Some(tracked) = dkg.session_nonce {
            if tracked != current {
                info!("DKG session nonce changed to: {}", current);
                dkg.session_nonce = Some(current);
                dkg.stage = None;
            }
        } else {
            info!("DKG session nonce set to: {}", current);
            dkg.session_nonce = Some(current);
        }
    }

    // Check DKG state changes.
    let stage = dkg.machine.stage();
    if let Some(tracked) = dkg.stage {
        if tracked != stage {
            info!("DKG stage changed to: {}", stage);
            dkg.stage = Some(stage);
        }
    } else {
        info!("DKG stage set to: {}", stage);
        dkg.stage = Some(stage);
    }
}

type SigningNoncesCommitmentsMap =
    Arc<Mutex<Option<Vec<(frost::round1::SigningNonces, frost::round1::SigningCommitments)>>>>;

/// The DKG state machine is responsible for managing the DKG process.
struct DkgState {
    // The DKG state machine
    machine: dkg::DkgStateMachine,
    // The current stage of the DKG process, used for logging purposes.
    stage: Option<dkg::Stage>,
    // The current session nonce, used for logging purposes.
    session_nonce: Option<u64>,
}

struct App<BitcoinRpcApi> {
    start_time: Instant,
    db: database::Db,
    btc_network: bitcoin::Network,
    pegout_scheduler: Mutex<PegoutScheduler>,
    /// This lock is taken when we're making a tx so that we don't accidentally
    /// spend the same operations twice.
    tx_lock: Arc<Mutex<()>>,
    identifier: frost::Identifier,
    #[cfg(test)]
    max_signers: u16,
    min_signers: u16,
    dkg: Mutex<Option<DkgState>>,
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
        telemetry: Option<Arc<Telemetry>>,
        btc_network: bitcoin::Network,
        identifier: u16,
    ) -> Result<PegoutScheduler, database::Error> {
        if let Some(latest) = db.get_pegout_mgr_finalized_block()? {
            let txs = db.get_tracked_txs()?;
            info!("Loaded pegout scheduler with {} pending txs", txs.len());
            Ok(PegoutScheduler::new(
                pegin_conf_depth,
                txs,
                latest,
                db.clone(),
                telemetry,
                btc_network,
                identifier,
            ))
        } else {
            info!("No finalized block found, using fallback checkpoint: {}", fallback_checkpoint);
            Ok(PegoutScheduler::new(
                pegin_conf_depth,
                vec![],
                fallback_checkpoint,
                db.clone(),
                telemetry,
                btc_network,
                identifier,
            ))
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

        // Prepare our Frost Id.
        let frost_identifier =
            frost::Identifier::derive(config.identifier.to_le_bytes().as_slice())
                .expect("valid identifier");

        info!("Local Frost identifier: {:?} - {:?}", config.identifier, frost_identifier);

        // Prepare coordinator Frost Id.
        let coordinator = if let Some(id) = config.coordinator {
            let i = frost_id!(id);
            info!("Specified Frost coordinator: {:?} - {:?}", id, i);
            i
        } else {
            let i = frost_id!(0);
            info!("Default Frost coordinator: {:?} - {:?}", 0, i);
            i
        };

        // Prepare the federation config.
        // TODO: Handle error
        let raw = std::fs::read_to_string(&config.federation_config_path)?;
        let federation = FederationTomlConfig::from_str(&raw)
            .map_err(|_| dkg::Error::BadConfig("invalid federation Toml config".to_string()))?;

        // Prepare our secret key.
        let raw = std::fs::read_to_string(&config.p2p_secret_key)?;
        let sanitzed_key = raw.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>();
        let secret_key = sanitzed_key
            .as_str()
            .parse::<secp256k1::SecretKey>()
            .map_err(|_| dkg::Error::BadConfig("invalid p2p secret key".to_string()))?;

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

        let fall_back_fee_rate =
            bitcoin::FeeRate::from_sat_per_vb(config.fall_back_fee_rate_sat_per_vbyte)
                .expect("valid fee rate");

        if let Some(telemetry) = telemetry.as_ref() {
            telemetry.update_transaction_fee_rates(
                config.btc_network,
                config.identifier,
                fall_back_fee_rate.to_sat_per_kwu() as f64,
            );

            if fall_back_fee_rate <= bitcoin::FeeRate::MIN {
                warn!("Fall back fee rate is below the minimum: {}", fall_back_fee_rate);
                telemetry.update_fee_rate_abnormalities(config.btc_network, config.identifier)
            }

            if fall_back_fee_rate >= bitcoin::FeeRate::MAX {
                warn!("Fall back fee rate is above the maximum: {}", fall_back_fee_rate);
                telemetry.update_fee_rate_abnormalities(config.btc_network, config.identifier)
            }
        }

        let pegin_confirmation_depth = get_pegin_confirmation_depth(config.btc_network);

        // update telemetry with the pegin confirmation depth
        if let Some(telemetry) = telemetry.as_ref() {
            telemetry.update_pegin_confirmation_depth(
                config.btc_network,
                config.identifier,
                pegin_confirmation_depth,
            );
        }

        let fallback_checkpoint = {
            let tip_height = measure_rpc_latency!(
                &telemetry,
                config.btc_network,
                config.identifier,
                "get_block_count",
                bitcoind_client.get_block_count()
            )
            .map_err(|e| Error::PegoutSchedulerSync(e.into()))?;

            measure_rpc_latency!(
                &telemetry,
                config.btc_network,
                config.identifier,
                "get_block_hash",
                bitcoind_client
                    .get_block_hash(tip_height.saturating_sub(pegin_confirmation_depth as u64))
            )
            .map_err(|e| Error::PegoutSchedulerSync(e.into()))?
        };
        let pegout_manager = Mutex::new(Self::load_pegout_scheduler(
            &db,
            fallback_checkpoint,
            pegin_confirmation_depth,
            telemetry.clone(),
            config.btc_network,
            config.identifier,
        )?);

        // NOTE (lamafab): in this implementation, the DKG state machine starts
        // automatically on startup if and only if no existing aggregated public
        // key is found in the db. In the future, when we're dealing with
        // dynamic Fed members, multiple multisigs and rotations, we'll need a
        // mechanism to start/stop the DKG process arbitrarily.
        let dkg = if db.get_key_package().expect("failed to interact with db").is_none() {
            warn!("No key package found, starting DKG process...");

            let dkg_config = dkg::Config {
                max_signers,
                min_signers,
                // NOTE: We set a very conservative timeout for the DKG process
                // to resend messages. For direct connections this could be set
                // to a lower millisecond range, technically.
                round1_package_timeout: Duration::from_secs(3),
                round2_package_timeout: Duration::from_secs(3),
                round3_package_timeout: Duration::from_secs(3),
                // Start a new DKG session if not completed in 5 minutes.
                pending_session_timeout: Some(Duration::from_secs(60 * 5)),
            };

            let mut members = BTreeMap::new();
            for (pos, fed_pubkey) in federation.federation_member_public_key.iter().enumerate() {
                let id = frost_id!(pos as u16);
                let pubkey = secp256k1::PublicKey::from_str(&fed_pubkey.key).map_err(|_| {
                    dkg::Error::BadConfig("invalid federation member public key".to_string())
                })?;

                members.insert(id, pubkey);
            }

            // As the coordinator, we simply use the system time as the session
            // nonce. This value doesn't need to be precisely synchronized - it
            // simply ensures that each server restart produces a higher nonce
            // than previous sessions, removing the need to persist this value
            // in the database.
            //
            // Non-coordinators automatically discard this value and use `None`,
            // and let the coordinator dictate the nonce.
            #[cfg(not(test))]
            let session_nonce = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("bad system time")
                .as_secs();

            #[cfg(test)]
            let session_nonce = 0;

            let machine = dkg::DkgStateMachine::new(
                frost_identifier,
                secret_key,
                coordinator,
                members,
                dkg_config,
                Some(session_nonce),
            )?;

            let state = DkgState { machine, stage: None, session_nonce: None };

            Mutex::new(Some(state))
        } else {
            Mutex::new(None)
        };

        Ok(Self {
            start_time: Instant::now(),
            btc_network: config.btc_network,
            db,
            pegout_scheduler: pegout_manager,
            tx_lock: Arc::new(Mutex::new(())),
            identifier: frost_identifier,
            dkg,
            frost_round1_nonces: Arc::new(Mutex::new(None)),
            config,
            btc_signing_server_jwt_secret,
            min_signers,
            #[cfg(test)]
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
        lock.sync_until(
            &self.bitcoind_client,
            checkpoint,
            &self.telemetry,
            self.btc_network,
            self.config.identifier,
        )?;
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
    // Define the associated type for the stream
    type GetFinalizedPegoutIdsStream =
        ReceiverStream<Result<rpc::GetFinalizedPegoutIdsResponse, tonic::Status>>;

    /* General Endpoints */
    async fn health_check(
        &self,
        request: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&request)?;
        info!("Health check request received");

        if let Some(telemetry) = self.telemetry.as_ref() {
            let is_bitcoind_syncing = is_syncing(
                &self.bitcoind_client,
                &self.telemetry,
                self.btc_network,
                self.config.identifier,
            )
            .ok()
            .unwrap_or(true);
            telemetry.update_health_check(
                self.btc_network,
                self.config.identifier,
                self.start_time.elapsed().as_secs(),
                &[("bitcoind", if is_bitcoind_syncing { "syncing" } else { "up" })],
            )
        };

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
                    timestamp: Some(p.timestamp),
                })
            })
            .collect::<Result<Vec<PegoutRequest>, tonic::Status>>();

        let pegouts = pegouts?;
        // Check pegouts are not in the finalized pegout ids list
        let finalized_pegout_ids: HashSet<_> =
            self.db.get_finalized_pegout_ids().to_status()?.iter().map(|p| p.id).collect();
        let pegouts_refs: Vec<&PegoutRequest> = pegouts
            .iter()
            .filter(|pegout| {
                if finalized_pegout_ids.contains(&pegout.id) {
                    error!("Received a pegout request for finalized id: {:?}", pegout.id);
                    return false;
                }
                true
            })
            .collect();

        self.db.store_pending_pegouts(&pegouts_refs).to_status()?;
        self.db.flush().to_status()?;
        info!("stored pegouts.len(): {:?}", pegouts.len());

        if let Some(telemetry) = self.telemetry.as_ref() {
            telemetry.update_pending_pegouts(
                self.btc_network,
                self.config.identifier,
                pegouts.len() as i64,
            );
        }

        Ok(tonic::Response::new(rpc::Empty {}))
    }

    async fn get_signing_status(
        &self,
        req: tonic::Request<rpc::GetSigningStatusRequest>,
    ) -> Result<tonic::Response<rpc::GetSigningStatusResponse>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let signing_session_id =
            handle_signing_error!(self, req, parse_signing_session_id(&req.signing_session_id));
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
        let res = tonic::Response::new(rpc::GetPendingPegoutsResponse {
            pending_pegouts: pending_pegouts
                .into_iter()
                .map(|p| rpc::PendingPegout {
                    pegout_id: p.id.as_bytes().to_vec(),
                    spk: p.spk.into_bytes().to_vec(),
                    amount: p.value.to_sat(),
                    height: p.botanix_height,
                    timestamp: p.timestamp.unwrap_or(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .expect("valid duration")
                            .as_secs(),
                    ),
                })
                .collect(),
        });
        Ok(res)
    }

    /// Returns all finalized pegout ids
    async fn get_finalized_pegout_ids(
        &self,
        req: tonic::Request<rpc::GetFinalizedPegoutIdsRequest>,
    ) -> Result<tonic::Response<Self::GetFinalizedPegoutIdsStream>, tonic::Status> {
        self.validate_jwt(&req)?;
        let db = self.db.clone();
        let request = req.into_inner();

        let (tx, rx) = tokio::sync::mpsc::channel(request.chunk_size as usize);
        info!(
            "get_finalized_pegout_ids: Starting stream task (chunk_size={})...",
            request.chunk_size
        );
        tokio::spawn(async move {
            let stream = db.get_finalized_pegout_ids_stream(request.chunk_size as usize);
            pin_mut!(stream);
            info!("get_finalized_pegout_ids stream task: Created DB stream.");

            while let Some(chunk_result) = stream.next().await {
                info!(
                    "get_finalized_pegout_ids stream task: Received chunk from DB stream: {:?}",
                    chunk_result
                );
                match chunk_result {
                    Ok((pegout_ids, chunk_index, total_chunks)) => {
                        let batch = rpc::GetFinalizedPegoutIdsResponse {
                            data: pegout_ids
                                .into_iter()
                                .map(|p| rpc::FinalizedPegout {
                                    id: p.id.as_bytes().to_vec(),
                                    botanix_block_height: p.block_number,
                                    botanix_block_timestamp: p.timestamp.unwrap_or(
                                        std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .expect("valid duration")
                                            .as_secs(),
                                    ),
                                })
                                .collect(),
                            chunk_index,
                            total_chunks,
                            is_final: chunk_index + 1 == total_chunks,
                        };

                        // send the batch with retries
                        info!("get_finalized_pegout_ids stream task: Sending chunk {}/{} with {} IDs to client.", chunk_index + 1, total_chunks, batch.data.len());
                        let fut = || async {
                            let tx = tx.clone();
                            let batch = batch.clone();
                            tx.send(Ok(batch)).await
                        };
                        if let Err(e) = retry_exec(
                            "sending_finalized_pegout_id_chunk",
                            fut,
                            3,
                            Duration::from_secs(2),
                        )
                        .await
                        {
                            error!("get_finalized_pegout_ids stream task: Client disconnected, stopping stream. Error = {:?}", e);
                            continue;
                        };

                        // add a small delay between chunks
                        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
                    }
                    Err(e) => {
                        error!("get_finalized_pegout_ids stream task: Error from DB stream: {}. Skipping chunk.", e);
                        // Optionally send error to client?
                        // if tx.send(Err(tonic::Status::internal(format!("DB Error: {}",
                        // e)))).await.is_err() { ... }
                        continue;
                    }
                }
            }
            info!("get_finalized_pegout_ids stream task: DB stream finished.");
        });

        Ok(tonic::Response::new(ReceiverStream::new(rx)))
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
        req: tonic::Request<rpc::ResetWalletStateRequest>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        info!("Received reset wallet state request");

        // handle finalized pegout ids
        let finalized_pegout_ids = req
            .finalized_pegout_ids
            .into_iter()
            .map(|v| {
                Ok(btcserverlib::database::FinalizedPegout {
                    id: PegoutId::from_bytes(&v.id)?,
                    block_number: v.botanix_block_height,
                    timestamp: Some(v.botanix_block_timestamp),
                })
            })
            .collect::<Result<Vec<btcserverlib::database::FinalizedPegout>, ()>>()
            .map_err(|_| internal!("Failed to convert finalized pegout ids"))?;
        let pegout_refs: Vec<&btcserverlib::database::FinalizedPegout> =
            finalized_pegout_ids.iter().collect();
        self.db.reset_finalized_pegout_ids(&pegout_refs).to_status()?;
        Ok(tonic::Response::new(rpc::Empty {}))
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
        let signing_session_id =
            handle_signing_error!(self, req, parse_signing_session_id(&req.signing_session_id));

        // Check if we have already provided nonces for the current session
        let mut nonces_lock = self.frost_round1_nonces.lock().await;
        if nonces_lock.is_some() {
            if let Some(telemetry) = self.telemetry.as_ref() {
                telemetry.update_signing_error_metrics(
                    self.btc_network,
                    self.config.identifier,
                    signing_session_id,
                    &SigningRound1Error::AlreadyInSigningSession.to_string(),
                );
            }
            return Err(tonic::Status::internal("Already in signing session"));
        }

        let mut psbt = match Psbt::deserialize(req.psbt.as_slice()).to_status() {
            Ok(psbt) => psbt,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_signing_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        req.signing_session_id.try_into().expect("signing session id"),
                        &e.to_string(),
                    );
                }
                return Err(e);
            }
        };

        let nonces = signer::get_round1_signing_package(
            &mut psbt,
            self.min_signers,
            &self.db,
            &self.identifier,
        )
        .map_err(SigningError::Round1)
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
        let signing_session_id =
            handle_signing_error!(self, req, parse_signing_session_id(&req.signing_session_id));

        let mut psbt = match Psbt::deserialize(req.psbt.as_slice()).to_status() {
            Ok(psbt) => psbt,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_signing_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        req.signing_session_id.try_into().expect("valid signing session id"),
                        &e.to_string(),
                    );
                }
                return Err(e);
            }
        };

        signer::get_round2_signing_package(
            &mut psbt,
            self.min_signers,
            &self.db,
            &self.identifier,
            &signing_nonces,
        )
        .map_err(|e| {
            if let Some(telemetry) = self.telemetry.as_ref() {
                telemetry.update_signing_error_metrics(
                    self.btc_network,
                    self.config.identifier,
                    signing_session_id,
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

        let signed_tx =
            psbt.clone().extract_tx().expect("just checked in get_round2_signing_package");

        // Note: the coordinator determines the pending pegouts to include in the psbt.
        // Signers may or may not have the same pending pegouts depending on their liveliness.
        // When signers sync with the network they will add the pending pegouts to their
        // database but some of them may have already been honored when they were offline.
        // Signers need to track the pending pegouts included in the psbt and clear the pending
        // pegouts from the database.

        // Extract pegout ids from the psbt to store with the tx
        if psbt.outputs.len() > UPPER_PEGOUT_BOUND {
            return Err(badarg!("Too many pegouts in the psbt"));
        }
        let mut psbt_pegout_ids: Vec<PegoutId> = Vec::with_capacity(psbt.outputs.len());
        info!("[get_round2_signing_package] Found {} outputs in the psbt", psbt.outputs.len());
        for output in psbt.outputs.iter() {
            if let Some(pegout_id) = output.pegout_id() {
                let pegout_id = PegoutId::from_bytes(&pegout_id)
                    .map_err(|_| {
                        SigningError::Round2(SigningRound2Error::FailedToDeserializePegoutId)
                    })
                    .to_status()?;
                psbt_pegout_ids.push(pegout_id);
            }
        }
        info!(
            "[get_round2_signing_package] Found {} pegout ids in the psbt",
            psbt_pegout_ids.len()
        );

        // Get the matching pending pegouts
        let pending_pegouts = match self.db.get_pending_pegouts().to_status() {
            Ok(pending_pegouts) => pending_pegouts,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_signing_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        req.signing_session_id.try_into().expect("valid signing session id"),
                        &e.to_string(),
                    );
                }
                return Err(e);
            }
        };

        info!(
            "[get_round2_signing_package] Found {} pending pegouts in the DB",
            pending_pegouts.len()
        );
        let psbt_pending_pegouts = pending_pegouts
            .into_iter()
            .filter(|p| psbt_pegout_ids.contains(&p.id))
            .collect::<Vec<_>>();

        info!(
            "[get_round2_signing_package] Found {} matching pending pegouts in the psbt",
            psbt_pending_pegouts.len()
        );
        self.add_tracked_tx(signed_tx.clone(), &psbt_pending_pegouts, SystemTime::now())
            .await
            .to_status()?;
        self.db.reset_pending_pegouts(&[]).to_status()?;
        self.db.flush().to_status()?;
        info!("[get_round2_signing_package] Pending pegouts removed and DB flushed.");
        // set the telemetry for pending pegouts back to 0
        if let Some(telemetry) = self.telemetry.as_ref() {
            telemetry.set_pending_pegouts(self.btc_network, self.config.identifier, 0_i64);
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
            handle_signing_error!(self, req, parse_signing_session_id(&req.signing_session_id));

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

        let tx_id = match measure_rpc_latency!(
            &self.telemetry,
            self.btc_network,
            self.config.identifier,
            "send_raw_transaction",
            self.bitcoind_client.send_raw_transaction(&tx)
        ) {
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

        let pegout_ids = psbt
            .pegout_ids()
            .iter()
            .map(|p| PegoutId::from_bytes(p).expect("values are 36 bytes"))
            .collect::<Vec<PegoutId>>();
        info!(
            "[finalize_signing] Removing {} pending pegouts from DB: {:?}",
            pegout_ids.len(),
            pegout_ids
        );
        self.db.remove_pending_pegout(&pegout_ids).to_status()?;
        // remove the pegouts from the telemetry gauge
        if let Some(telemetry) = self.telemetry.as_ref() {
            telemetry.update_pending_pegouts(
                self.btc_network,
                self.config.identifier,
                -(pegout_ids.len() as i64),
            );
        }
        self.db.flush().to_status()?;

        if let Some(tx_id) = tx_id {
            info!("Broadcasted tx: {:?}", tx_id);
        } else {
            info!("Transaction already broadcasted and in pool");
        }

        let psbt_bytes = hex::decode(psbt.serialize_hex())
            .map_err(|e| internal!("Failed to serialize psbt: {}", e))?;

        // mark the signing session as finalized
        if let Some(telemetry) = self.telemetry.as_ref() {
            telemetry.record_finalized_signing_sessions(self.btc_network, self.config.identifier);
            telemetry.update_signing_success_rate_metrics(
                self.btc_network,
                self.config.identifier,
                signing_session_id,
            )
        }

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

        let req = req.into_inner();

        if let Err(e) = self.db.get_key_package().to_status() {
            if let Some(telemetry) = self.telemetry.as_ref() {
                telemetry.update_signing_error_metrics(
                    self.btc_network,
                    self.config.identifier,
                    req.signing_session_id.try_into().expect("signing session id is valid"),
                    &e.to_string(),
                );
            }
            return Err(e);
        };

        // take a lock on the tx_lock
        let _tx_lock = self.tx_lock.lock();

        info!(
            "Received make tx request for signing session id: {:?}",
            hex::encode(req.signing_session_id.clone())
        );
        let signing_session_id =
            handle_signing_error!(self, req, parse_signing_session_id(&req.signing_session_id));

        let checkpoint = match BlockHash::from_slice(&req.checkpoint_block_hash) {
            Ok(checkpoint) => checkpoint,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_signing_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        req.signing_session_id.try_into().expect("valid signing session id"),
                        &e.to_string(),
                    );
                }
                return Err(badarg!("invalid checkpoint hash: {}", e));
            }
        };

        let fee_res = measure_rpc_latency!(
            &self.telemetry,
            self.btc_network,
            self.config.identifier,
            "estimate_smart_fee",
            self.bitcoind_client
                .estimate_smart_fee(1, Some(bitcoincore_rpc::json::EstimateMode::Conservative))
        );

        let mut fee_rate = self.fall_back_fee_rate;
        if let Ok(fee) = fee_res {
            if let Some(f) = fee.fee_rate {
                fee_rate = btc_per_kb_to_sat_per_vb(f);
            }
        }

        if let Some(telemetry) = self.telemetry.as_ref() {
            telemetry.update_transaction_fee_rates(
                self.btc_network,
                self.config.identifier,
                fee_rate.to_sat_per_kwu() as f64,
            );

            if fee_rate <= bitcoin::FeeRate::MIN {
                warn!("Fee rate is below the minimum: {}", fee_rate);
                telemetry.update_fee_rate_abnormalities(self.btc_network, self.config.identifier)
            }

            if fee_rate >= bitcoin::FeeRate::MAX {
                warn!("Fee rate is above the maximum: {}", fee_rate);
                telemetry.update_fee_rate_abnormalities(self.btc_network, self.config.identifier)
            }
        }

        debug!("Cord Fee rate: {:?}", fee_rate);

        // First sync the pegout scheduler as this may add tracked pegouts back to the pending
        // pegouts list
        handle_signing_error!(
            self,
            signing_session_id,
            self.sync_pegout_scheduler(checkpoint).await,
            check_only
        );

        let tracked_txs = match self.db.get_tracked_txs().to_status() {
            Ok(tracked_txs) => tracked_txs,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_signing_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        req.signing_session_id.try_into().expect("valid signing session id"),
                        &e.to_string(),
                    );
                }
                return Err(e);
            }
        };

        // Select up to `UPPER_PEGOUT_BOUND` pegouts, sorted by age in ascending
        // order. Respectively, the oldest pegouts come first.
        let pending_pegouts = match self.db.coord_pending_pegouts(UPPER_PEGOUT_BOUND).to_status() {
            Ok(pending_pegouts) => pending_pegouts,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_signing_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        req.signing_session_id.try_into().expect("valid signing session id"),
                        &e.to_string(),
                    );
                }
                return Err(e);
            }
        };

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

        let psbt = match coordinator::make_tx(
            outputs,
            fee_rate,
            change_script,
            &self.db,
            self.min_signers,
            tracked_txs,
            self.telemetry.clone(),
            self.btc_network,
            self.config.identifier,
        )
        .to_status()
        {
            Ok(psbt) => psbt,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_signing_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        req.signing_session_id.try_into().expect("valid signing session id"),
                        &e.to_string(),
                    );
                }
                return Err(e);
            }
        };

        // Log the outputs of the generated PSBT for debugging
        info!("PSBT generated by make_tx for session {:?}:", hex::encode(signing_session_id));
        // Iterate through the outputs of the unsigned transaction embedded in the PSBT
        for (i, tx_output) in psbt.unsigned_tx.output.iter().enumerate() {
            info!(
                "- Output {}: value={}, script_pubkey={:?}",
                i, tx_output.value, tx_output.script_pubkey
            );
        }
        // Note: Standard PSBT doesn't explicitly track change_index in rust-bitcoin library easily.
        // We rely on our logic correctly identifying it later.

        // Save psbt to db
        handle_signing_error!(
            self,
            req.signing_session_id.try_into().expect("valid signing session id"),
            self.db.update_psbt(&signing_session_id, &psbt),
            check_only
        );

        self.db.flush().to_status()?;

        let psbt_bytes = match hex::decode(psbt.serialize_hex()).to_status() {
            Ok(psbt_bytes) => psbt_bytes,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_signing_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        req.signing_session_id.try_into().expect("valid signing session id"),
                        &e.to_string(),
                    );
                }
                return Err(e);
            }
        };

        let res = tonic::Response::new(rpc::SigningPackage {
            // identifier really doent matter here.
            identifier: self.identifier.serialize().to_vec(),
            psbt: psbt_bytes,
            signing_session_id: signing_session_id.to_vec(),
        });

        // record the new signing session in telemetry
        if let Some(telemetry) = self.telemetry.as_ref() {
            telemetry.record_total_signing_sessions(self.btc_network, self.config.identifier);
        }

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
        let signing_session_id =
            handle_signing_error!(self, req, parse_signing_session_id(&req.signing_session_id));

        let psbt = match coordinator::get_to_sign(&signing_session_id, &self.db, self.min_signers)
            .to_status()
        {
            Ok(psbt) => psbt,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_signing_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        req.signing_session_id.try_into().expect("valid signing session id"),
                        &e.to_string(),
                    );
                }
                return Err(e);
            }
        };

        let psbt_bytes = match hex::decode(psbt.serialize_hex()) {
            Ok(psbt_bytes) => psbt_bytes,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_signing_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        req.signing_session_id.try_into().expect("valid signing session id"),
                        &e.to_string(),
                    );
                }
                return Err(internal!("Failed to serialize psbt: {}", e));
            }
        };

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
        let start = Instant::now();
        self.validate_jwt(&req)?;

        let req = req.into_inner();

        // Ensure we have a key package
        if let Err(e) = self.db.get_key_package().to_status() {
            if let Some(telemetry) = self.telemetry.as_ref() {
                telemetry.update_signing_error_metrics(
                    self.btc_network,
                    self.config.identifier,
                    req.signing_session_id.try_into().expect("signing session id is valid"),
                    &e.to_string(),
                );
            }
            return Err(e);
        };

        info!(
            "Received new round1 signing package for signing session id: {:?}",
            hex::encode(req.signing_session_id.clone())
        );
        let signing_session_id =
            handle_signing_error!(self, req, parse_signing_session_id(&req.signing_session_id));

        let frost_id = handle_signing_error!(self, req, deserialize_frost_peer_id(req.identifier));

        let psbt = match Psbt::deserialize(req.psbt.as_slice()).to_status() {
            Ok(psbt) => psbt,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_signing_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        req.signing_session_id.try_into().expect("signing session id is valid"),
                        &e.to_string(),
                    );
                }
                return Err(e);
            }
        };

        if let Err(e) = coordinator::add_round1_signing(
            &signing_session_id,
            frost_id,
            &psbt,
            &self.db,
            self.min_signers,
        )
        .to_status()
        {
            if let Some(telemetry) = self.telemetry.as_ref() {
                telemetry.update_signing_error_metrics(
                    self.btc_network,
                    self.config.identifier,
                    req.signing_session_id.try_into().expect("signing session id is valid"),
                    &e.to_string(),
                );
            }
            return Err(e);
        };

        if let Some(telemetry) = self.telemetry.as_ref() {
            telemetry.update_round1_signing_metrics(
                self.btc_network,
                self.config.identifier,
                &signing_session_id,
                req.psbt.as_slice().len(),
                start.elapsed().as_millis(),
            )
        }

        Ok(tonic::Response::new(rpc::Empty {}))
    }

    async fn new_round2_signing_package(
        &self,
        req: tonic::Request<rpc::SigningPackage>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        let start = Instant::now();
        self.validate_jwt(&req)?;

        let req = req.into_inner();
        // Ensure we have a key package
        if let Err(e) = self.db.get_key_package().to_status() {
            if let Some(telemetry) = self.telemetry.as_ref() {
                telemetry.update_signing_error_metrics(
                    self.btc_network,
                    self.config.identifier,
                    req.signing_session_id.try_into().expect("signing session id is valid"),
                    &e.to_string(),
                );
            }
            return Err(e);
        };

        info!("Received round2 signing package");
        let signing_session_id =
            handle_signing_error!(self, req, parse_signing_session_id(&req.signing_session_id));

        let frost_id = handle_signing_error!(self, req, deserialize_frost_peer_id(req.identifier));

        let psbt = match Psbt::deserialize(req.psbt.as_slice()).to_status() {
            Ok(psbt) => psbt,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_signing_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        req.signing_session_id.try_into().expect("signing session id is valid"),
                        &e.to_string(),
                    );
                }
                return Err(e);
            }
        };

        if let Err(e) = coordinator::add_round2_signing(
            &signing_session_id,
            frost_id,
            &psbt,
            &self.db,
            self.min_signers,
        )
        .to_status()
        {
            if let Some(telemetry) = self.telemetry.as_ref() {
                telemetry.update_signing_error_metrics(
                    self.btc_network,
                    self.config.identifier,
                    req.signing_session_id.try_into().expect("signing session id is valid"),
                    &e.to_string(),
                );
            }
            return Err(e);
        }

        if let Some(telemetry) = self.telemetry.as_ref() {
            telemetry.update_round2_signing_metrics(
                self.btc_network,
                self.config.identifier,
                &signing_session_id,
                req.psbt.as_slice().len(),
                start.elapsed().as_millis(),
            )
        }

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

    async fn get_dkg_payloads(
        &self,
        req: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::DkgPayloads>, tonic::Status> {
        self.validate_jwt(&req)?;

        if self.db.get_key_package().to_status()?.is_some() {
            return Err(already_exists!("already have key package"));
        }

        let mut l = self.dkg.lock().await;
        let Some(dkg) = l.as_mut() else {
            return Err(tonic::Status::internal("dkg not initialized"));
        };

        // Generate responses on potential timeout events; if no timers expired,
        // then this is simply a no-op call.
        dkg.machine.on_timeout(Instant::now());

        // Logging DKG state.
        print_dkg_state_log(dkg);

        // Process response (or initial) payloads.
        let mut payloads = vec![];
        while let Some(p) = dkg.machine.send(Instant::now()) {
            // Encode the payload.
            let mut bytes = vec![];
            ciborium::into_writer(&p.msg, &mut bytes).expect("failed to encode Dkg payload");

            payloads.push(rpc::DkgPayload {
                sender: p.sender.serialize(),
                recipient: p.recipient.serialize(),
                payload: bytes.clone(),
            });
        }

        // Set any timers, and retrieve next timeout event.
        let timeout = dkg.machine.timeout(Instant::now());

        let resp = rpc::DkgPayloads {
            // TODO (lamafab): Option?
            timeout: timeout.map(|t| t.as_millis() as u64).unwrap_or(u64::MAX),
            payloads,
        };

        Ok(tonic::Response::new(resp))
    }

    async fn new_dkg_payload(
        &self,
        req: tonic::Request<rpc::DkgPayload>,
    ) -> Result<tonic::Response<rpc::DkgPayloads>, tonic::Status> {
        self.validate_jwt(&req)?;
        let start = Instant::now();
        // NOTE: we do not make an existing aggregated key check at the start
        // here, since we still want to respond to the coordinator in case they
        // have not received our acknowledgment yet.

        let req = req.into_inner();
        let sender = match deserialize_frost_peer_id(req.sender.clone()).to_status() {
            Ok(sender) => sender,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_dkg_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        &e.to_string(),
                    );
                }
                return Err(e);
            }
        };

        let recipient = match deserialize_frost_peer_id(req.recipient.clone()).to_status() {
            Ok(recipient) => recipient,
            Err(e) => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_dkg_error_metrics(
                        self.btc_network,
                        self.config.identifier,
                        &e.to_string(),
                    );
                }
                return Err(e);
            }
        };

        // Decode the payload.
        let msg =
            match ciborium::from_reader::<dkg::DkgMessage, _>(req.payload.as_slice()).to_status() {
                Ok(msg) => msg,
                Err(e) => {
                    if let Some(telemetry) = self.telemetry.as_ref() {
                        telemetry.update_dkg_error_metrics(
                            self.btc_network,
                            self.config.identifier,
                            &e.to_string(),
                        );
                    }
                    return Err(e);
                }
            };

        let payload = dkg::DkgPayload { sender, recipient, msg };

        match &payload.msg {
            dkg::DkgMessage::Round1 {
                initiator: _,
                context: _,
                nonce: _,
                ephemeral_pub: _,
                signature: _,
                package,
            } => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    let mut bytes = vec![];
                    ciborium::into_writer(&package, &mut bytes)
                        .expect("failed to encode Dkg payload");
                    telemetry.update_round1_dkg_metrics(
                        self.btc_network,
                        self.config.identifier,
                        bytes.len(),
                        start.elapsed().as_millis(),
                    );
                }
            }
            dkg::DkgMessage::Round2 { initiator: _, target: _, nonce: _, package } => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    let mut bytes = vec![];
                    ciborium::into_writer(&package, &mut bytes)
                        .expect("failed to encode Dkg payload");
                    telemetry.update_round2_dkg_metrics(
                        self.btc_network,
                        self.config.identifier,
                        bytes.len(),
                        start.elapsed().as_millis(),
                    );
                }
            }
            dkg::DkgMessage::Round3 { initiator: _, signature: _ } => {
                if let Some(telemetry) = self.telemetry.as_ref() {
                    telemetry.update_round3_dkg_metrics(
                        self.btc_network,
                        self.config.identifier,
                        start.elapsed().as_millis(),
                    );
                }
            }
            _ => {}
        }

        // Acquire the lock on the dkg machine.
        let mut l = self.dkg.lock().await;
        let Some(dkg) = l.as_mut() else {
            if let Some(telemetry) = self.telemetry.as_ref() {
                telemetry.update_dkg_error_metrics(
                    self.btc_network,
                    self.config.identifier,
                    "dkg not initialized",
                );
            }
            return Err(tonic::Status::internal("dkg not initialized"));
        };

        // Process the payload.
        if let Err(e) = dkg.machine.recv(payload).to_status() {
            if let Some(telemetry) = self.telemetry.as_ref() {
                telemetry.update_dkg_error_metrics(
                    self.btc_network,
                    self.config.identifier,
                    &e.to_string(),
                );
            }
            return Err(e);
        }

        // Logging DKG state.
        print_dkg_state_log(dkg);

        // Process response (or initial) payloads.
        let mut payloads = vec![];
        while let Some(p) = dkg.machine.send(Instant::now()) {
            // Encode the payload.
            let mut bytes = vec![];
            ciborium::into_writer(&p.msg, &mut bytes).expect("failed to encode Dkg payload");

            payloads.push(rpc::DkgPayload {
                sender: p.sender.serialize(),
                recipient: p.recipient.serialize(),
                payload: bytes.clone(),
            });
        }

        if let Some((sec_key, pub_key)) = dkg.machine.aggregate_key_packages() {
            if self.db.get_key_package().to_status()?.is_none() {
                info!("DKG completed successfully, saving key packages...");
                if let Err(e) = self.db.set_key_package(sec_key.clone()).to_status() {
                    if let Some(telemetry) = self.telemetry.as_ref() {
                        telemetry.update_dkg_error_metrics(
                            self.btc_network,
                            self.config.identifier,
                            &e.to_string(),
                        );
                    }
                    return Err(e);
                }

                if let Err(e) = self.db.set_pubkey_package(pub_key.clone()).to_status() {
                    if let Some(telemetry) = self.telemetry.as_ref() {
                        telemetry.update_dkg_error_metrics(
                            self.btc_network,
                            self.config.identifier,
                            &e.to_string(),
                        );
                    }
                    return Err(e);
                }
                if let Err(e) = self.db.flush().to_status() {
                    if let Some(telemetry) = self.telemetry.as_ref() {
                        telemetry.update_dkg_error_metrics(
                            self.btc_network,
                            self.config.identifier,
                            &e.to_string(),
                        );
                    }
                    return Err(e);
                }

                // Note that we keep the dkg machine running, in case the
                // coordinator does not receive the final acknowledgment and we need
                // to issue a response.
                //
                // TODO (lamafab): we could technically shut it down once we receive
                // the first signing request, since that indicates that the Dkg
                // process has completed successfully. But there are no downsides of
                // keeping it running as of now.
            }
        }

        // Set any timers, and retrieve next timeout event.
        let timeout = dkg.machine.timeout(Instant::now());

        let resp = rpc::DkgPayloads {
            // TODO (lamafab): Option?
            timeout: timeout.map(|t| t.as_millis() as u64).unwrap_or(u64::MAX),
            payloads,
        };

        Ok(tonic::Response::new(resp))
    }

    // Currently not used
    async fn signer_finalize(
        &self,
        _req: tonic::Request<rpc::FinalizeSignerRequest>,
    ) -> Result<tonic::Response<rpc::FinalizeSigningResponse>, tonic::Status> {
        panic!("Not used");
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
            error!("Grpc server: Join Error {}", err);
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
    use bitcoin::{secp256k1, OutPoint, Script, Txid};
    use btcserverlib::dkg::DkgMessage;
    use frost_secp256k1_tr::keys::dkg::round1;
    use rand::{thread_rng, Rng};
    use std::{str::FromStr, vec};
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
        let temp_db = TempDir::new().unwrap();

        // WARNING: This is a test federation config with exposed private keys,
        // DO NOT use in production!
        let federation_content = r#"botanix-fee-recipient = ""
        minting-contract-bytecode = ""
        lst-fee-receiver = ""

        [[federation-member-public-key]]
        key = "03185b1f0226d6d5949b902f083dd6e5b04ecdccdedd4cf48080de60b0bfe3b606"
        # Private key: 46de0f5cdbf2619ba8155964f951661ef89126aaddfcbbab56b7422e37572ff8
        socket-addr = "127.0.0.1:30303"

        [[federation-member-public-key]]
        key = "038df7fcb0e1cdd68741ca85184e046a42c914e0c3ffcb2464d46be3d8b4a5b140"
        # Private key: 27eeb2264674f15f2bac84d84b5e8f0c40722f8327fe7354bf14c84e248f8838
        socket-addr = "127.0.0.1:30304"

        [[federation-member-public-key]]
        key = "02a7a1a9c37cd072f9752ef6b154876fe51f1ad2f7a6a627ef26e5075631af9f29"
        socket-addr = "127.0.0.1:30305"
        "#;

        let mut temp_federation = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut temp_federation, federation_content.as_bytes()).unwrap();

        let secret_key_content = "46de0f5cdbf2619ba8155964f951661ef89126aaddfcbbab56b7422e37572ff8";
        let mut temp_secret_key = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut temp_secret_key, secret_key_content.as_bytes()).unwrap();

        let bitcoind_client = MockBitcoind::new();

        let config = Config {
            db: temp_db.into_path(),
            btc_network: bitcoin::Network::Regtest,
            identifier: 0,
            coordinator: Some(0),
            federation_config_path: temp_federation.path().to_owned(),
            p2p_secret_key: temp_secret_key.path().to_owned(),
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

        // Keep the temp files alive for the duration of the test
        std::mem::forget(temp_federation);
        std::mem::forget(temp_secret_key);

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
    async fn dkg_should_work_if_missing_key_package() {
        let app = setup().await;
        let req = tonic::Request::new(rpc::Empty {});
        let payloads = app.get_dkg_payloads(req).await.unwrap();
        let inner = payloads.into_inner();

        for payload in inner.payloads {
            let frost_id = deserialize_frost_peer_id(payload.sender).unwrap();
            assert_eq!(frost_id, frost_id!(0));
            let payload = payload.payload;
            let msg: DkgMessage = ciborium::from_reader(payload.as_slice()).unwrap();
            match msg {
                DkgMessage::Round1 { .. } => {}
                _ => panic!("Expected Round1 message"),
            }
        }
        // Not much to assert on here, just that we can deserialize the package
    }

    #[tokio::test]
    async fn dkg_should_should_retry_when_no_response() {
        let app = setup().await;

        // Two payloads to be sent.
        let req = tonic::Request::new(rpc::Empty {});
        let payloads = app.get_dkg_payloads(req).await.unwrap();
        let inner = payloads.into_inner();
        assert_eq!(inner.payloads.len(), 2);

        // No payloads to be sent right now.
        let req = tonic::Request::new(rpc::Empty {});
        let payloads = app.get_dkg_payloads(req).await.unwrap();
        let inner = payloads.into_inner();
        assert!(inner.payloads.is_empty());

        // Wait until `get_dkg_payloads` should be called again.
        assert!(inner.timeout > 0);
        let timeout = Duration::from_millis(inner.timeout);
        tokio::time::sleep(timeout).await;

        // Two payloads to be (re-)sent.
        let req = tonic::Request::new(rpc::Empty {});
        let payloads = app.get_dkg_payloads(req).await.unwrap();
        let inner = payloads.into_inner();
        assert_eq!(inner.payloads.len(), 2);
    }

    /// Test the basic DKG interface. More comprehensive tests are covered
    /// separately.
    #[tokio::test]
    async fn basic_dkg_interface() {
        const SAMPLE_ROUND_PKG: &[u8] = &[
            0, 35, 15, 138, 179, 2, 2, 120, 88, 85, 71, 235, 157, 87, 39, 38, 125, 191, 226, 130,
            130, 109, 33, 101, 203, 186, 92, 8, 192, 49, 14, 162, 200, 99, 210, 81, 193, 116, 35,
            3, 3, 106, 54, 33, 158, 157, 204, 101, 31, 134, 240, 213, 83, 120, 7, 193, 132, 135, 1,
            209, 27, 29, 108, 85, 16, 2, 41, 11, 129, 48, 199, 108, 64, 82, 233, 151, 145, 38, 39,
            23, 230, 84, 196, 216, 128, 145, 22, 182, 69, 191, 243, 11, 111, 220, 94, 34, 101, 66,
            1, 34, 206, 187, 151, 84, 248, 127, 11, 173, 110, 104, 72, 32, 73, 170, 148, 211, 170,
            108, 244, 232, 37, 117, 104, 172, 111, 16, 249, 70, 33, 22, 18, 156, 178, 255, 134, 99,
            134,
        ];

        const SAMPLE_EPH_PUB: &[u8] = &[
            3, 132, 131, 44, 133, 229, 63, 171, 246, 209, 196, 34, 121, 0, 121, 231, 3, 132, 160,
            221, 29, 145, 119, 9, 4, 200, 46, 76, 45, 21, 99, 42, 11,
        ];

        // Sample signature generated with private key:
        // 27eeb2264674f15f2bac84d84b5e8f0c40722f8327fe7354bf14c84e248f8838
        //
        // Corresponding public key:
        // 038df7fcb0e1cdd68741ca85184e046a42c914e0c3ffcb2464d46be3d8b4a5b140
        //
        // Respectively, the second entry in the temporary federation config.
        const SAMPLE_SIG: &[u8] = &[
            82, 169, 233, 140, 210, 93, 174, 189, 154, 236, 130, 97, 121, 221, 140, 74, 98, 56,
            114, 223, 112, 103, 88, 29, 209, 127, 21, 46, 128, 93, 97, 170, 15, 165, 91, 19, 97,
            103, 12, 84, 50, 209, 217, 240, 124, 55, 62, 188, 29, 90, 73, 22, 206, 224, 205, 49,
            218, 85, 134, 54, 192, 124, 24, 125,
        ];

        // Setup Alice (coordinator), Bob, and Eve.
        let app = setup().await;

        let round1_pkg = round1::Package::deserialize(SAMPLE_ROUND_PKG).unwrap();
        let ephemeral_pub = secp256k1::PublicKey::from_slice(SAMPLE_EPH_PUB).unwrap();
        let signature = secp256k1::ecdsa::Signature::from_compact(SAMPLE_SIG).unwrap();

        // Alice generates two packages, one for Bob and one for Eve.
        {
            let req = tonic::Request::new(rpc::Empty {});
            let payloads = app.get_dkg_payloads(req).await.unwrap();
            let inner = payloads.into_inner();
            assert_eq!(inner.payloads.len(), 2);

            let p1 = &inner.payloads[0];
            let msg: DkgMessage = ciborium::from_reader(p1.payload.as_slice()).unwrap();
            let DkgMessage::Round1 { context, nonce, .. } = msg else {
                panic!("Expected Round1 message");
            };

            assert_eq!(context, dkg::SESSION_CONTEXT);
            assert_eq!(nonce, 0);
            //
            assert_eq!(p1.sender, frost_id!(0).serialize());
            assert_eq!(p1.recipient, frost_id!(2).serialize());

            let p2 = &inner.payloads[1];
            let msg: DkgMessage = ciborium::from_reader(p2.payload.as_slice()).unwrap();
            let DkgMessage::Round1 { context, nonce, .. } = msg else {
                panic!("Expected Round1 message");
            };

            assert_eq!(context, dkg::SESSION_CONTEXT);
            assert_eq!(nonce, 0);
            //
            assert_eq!(p2.sender, frost_id!(0).serialize());
            assert_eq!(p2.recipient, frost_id!(1).serialize());
        };

        // Bob sends his round1 package to Alice.
        {
            // We use a temporary structure that that not contain the _Sealed_
            // newtype for `round1::Package`, as seen in `DkgMessage`.
            #[derive(serde::Serialize, serde::Deserialize)]
            enum Embedded {
                Round1 {
                    context: Vec<u8>,
                    nonce: u64,
                    initiator: frost::Identifier,
                    ephemeral_pub: secp256k1::PublicKey,
                    signature: secp256k1::ecdsa::Signature,
                    package: round1::Package,
                },
            }

            let msg = Embedded::Round1 {
                context: dkg::SESSION_CONTEXT.to_vec(),
                nonce: 0,
                initiator: frost_id!(1),
                ephemeral_pub,
                signature,
                package: round1_pkg,
            };

            let mut payload = vec![];
            ciborium::into_writer(&msg, &mut payload).unwrap();

            let req = tonic::Request::new(rpc::DkgPayload {
                sender: frost_id!(1).serialize().to_vec(),
                recipient: frost_id!(0).serialize().to_vec(),
                payload,
            });

            let resp = app.new_dkg_payload(req).await.unwrap();
            let inner = resp.into_inner();

            // Alice responds with Ack to Bob and forwards Bob's package to Eve.
            assert_eq!(inner.payloads.len(), 2);

            let p1 = &inner.payloads[0];
            let msg: DkgMessage = ciborium::from_reader(p1.payload.as_slice()).unwrap();
            let DkgMessage::AckRound1 { .. } = msg else {
                panic!("Expected AckRound1 message");
            };

            assert_eq!(p1.sender, frost_id!(0).serialize());
            assert_eq!(p1.recipient, frost_id!(1).serialize());

            let p2 = &inner.payloads[1];
            let msg: DkgMessage = ciborium::from_reader(p2.payload.as_slice()).unwrap();
            let DkgMessage::Round1 { .. } = msg else {
                panic!("Expected Round1 message");
            };

            assert_eq!(p2.sender, frost_id!(0).serialize());
            assert_eq!(p2.recipient, frost_id!(2).serialize());
        }
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
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("valid duration")
                    .as_secs() as u64,
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

    #[tokio::test]
    async fn test_new_consensus_checkpoint_no_finalized_pegouts_stored() {
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
        let req = tonic::Request::new(rpc::ConsensusCheckpointRequest {
            checkpoint_block_hash: BlockHash::all_zeros().to_byte_array().to_vec(),
            pegins: pegins.clone(),
            pending_pegouts: vec![],
        });
        let _res = app.new_consensus_checkpoint(req).await.unwrap();

        // Store finalized pegout
        let mut rng = thread_rng();
        let pegout_id = PegoutId::new(rng.gen::<[u8; 32]>(), 0);
        let finalized_pegout = btcserverlib::database::FinalizedPegout {
            id: pegout_id,
            block_number: 100,
            timestamp: None,
        };
        app.db.store_finalized_pegout_ids(&[&finalized_pegout]).expect("valid finalized pegout");

        // Try and store a pending pegout with the the finalized pegout id
        let pending_pegout = rpc::PendingPegout {
            pegout_id: finalized_pegout.id.as_bytes().to_vec(),
            spk: random_p2wpkh_script().as_bytes().to_vec(),
            amount: 100_000, // sats
            height: 1,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("valid duration")
                .as_secs(),
        };
        let req = tonic::Request::new(rpc::ConsensusCheckpointRequest {
            checkpoint_block_hash: BlockHash::all_zeros().to_byte_array().to_vec(),
            pegins: vec![],
            pending_pegouts: vec![pending_pegout],
        });
        let _res = app.new_consensus_checkpoint(req).await.unwrap();

        let pending_pegouts = app.db.get_pending_pegouts().expect("valid pending pegouts");
        assert!(pending_pegouts.is_empty(), "No pending pegouts should be stored");
    }

    #[tokio::test]
    async fn test_finalized_pegout_ids_streaming_chunksize_gt_chunks() {
        let app = setup().await;
        let num_txs = 52;
        let mut finalized_pegout_ids = vec![];
        let mut rng = thread_rng();
        for i in 0..num_txs {
            let pegout_id = PegoutId::new(rng.gen::<[u8; 32]>(), i as u32);
            let finalized_pegout = btcserverlib::database::FinalizedPegout {
                id: pegout_id,
                block_number: 100,
                timestamp: None,
            };
            finalized_pegout_ids.push(finalized_pegout);
        }
        let finalized_pegout_ids_slice =
            finalized_pegout_ids.iter().collect::<Vec<&btcserverlib::database::FinalizedPegout>>();
        app.db.store_finalized_pegout_ids(&finalized_pegout_ids_slice).unwrap();

        let chunk_size = 10;
        let req = tonic::Request::new(rpc::GetFinalizedPegoutIdsRequest { chunk_size });
        let res = app.get_finalized_pegout_ids(req).await.unwrap();
        let mut stream = res.into_inner();
        let mut collected_chunks = vec![];
        while let Some(item) = stream.next().await {
            let item = item.unwrap();
            assert_eq!(item.total_chunks, (num_txs as u64).div_ceil(chunk_size));
            collected_chunks.extend_from_slice(&item.data);
        }
        assert_eq!(collected_chunks.len(), num_txs);
    }

    #[tokio::test]
    async fn test_finalized_pegout_ids_streaming_chunksize_lt_chunks() {
        let app = setup().await;
        let num_txs = 1;
        let mut finalized_pegout_ids = vec![];
        let mut rng = thread_rng();
        for i in 0..num_txs {
            let pegout_id = PegoutId::new(rng.gen::<[u8; 32]>(), i as u32);
            let finalized_pegout = btcserverlib::database::FinalizedPegout {
                id: pegout_id,
                block_number: 100,
                timestamp: None,
            };
            finalized_pegout_ids.push(finalized_pegout);
        }
        let finalized_pegout_ids_slice =
            finalized_pegout_ids.iter().collect::<Vec<&btcserverlib::database::FinalizedPegout>>();
        app.db.store_finalized_pegout_ids(&finalized_pegout_ids_slice).unwrap();

        let chunk_size = 10;
        let req = tonic::Request::new(rpc::GetFinalizedPegoutIdsRequest { chunk_size });
        let res = app.get_finalized_pegout_ids(req).await.unwrap();
        let mut stream = res.into_inner();
        let mut collected_chunks = vec![];
        while let Some(item) = stream.next().await {
            let item = item.unwrap();
            assert_eq!(item.total_chunks, (num_txs as u64).div_ceil(chunk_size));
            collected_chunks.extend_from_slice(&item.data);
        }
        assert_eq!(collected_chunks.len(), num_txs);
    }
}
