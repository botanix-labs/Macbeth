#[macro_use]
extern crate log;

use std::{
    collections::{BTreeMap, HashSet},
    fmt::Debug,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use alloy_rpc_types_engine::{JwtError, JwtSecret};
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
    database::{self},
    dkg,
    federation_args::FederationTomlConfig,
    frost_id,
    http::{create_web_server, state::ServerState},
    merkle::get_wallet_state_commitment,
    pegout_id::PegoutId,
    pegout_scheduler::{self, PegoutRequest, PegoutScheduler},
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
        address::{
            generate_taproot_address, generate_taproot_scriptpubkey, generate_tweaked_public_key,
        },
        psbt::{PsbtExt, PsbtOutputExt},
        util::VerifyingKeyExt,
    },
};
use file_descriptor::FILE_DESCRIPTOR_SET;
use frost_secp256k1_tr as frost;
use futures::{pin_mut, StreamExt};
use futures_util::future::FutureExt;
use thiserror::Error;
use tokio::sync::{oneshot, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{codegen::CompressionEncoding, metadata::BinaryMetadataKey, transport::Server};

const JWT_HEADER_KEY: &str = "trace-proto-bin";
const DEFAULT_COORDINATOR_ID: u16 = 0;

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
            let i = frost_id!(DEFAULT_COORDINATOR_ID);
            info!("Default Frost coordinator: {:?} - {:?}", DEFAULT_COORDINATOR_ID, i);
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

        info!("excluded eth addresses len = {:?}", config.excluded_eth_addresses.len());

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

    pub fn is_coordinator(&self) -> bool {
        let coordinator_id = self.config.coordinator.unwrap_or(DEFAULT_COORDINATOR_ID);
        self.config.identifier == coordinator_id
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
        // Check pegouts are not in the finalized pegout ids list or are tracked by the Pegout
        // Scheduler
        let mut broadcasted_pegout_ids: HashSet<_> =
            self.db.get_finalized_pegout_ids().to_status()?.iter().map(|p| p.id).collect();
        // Get Pegout Scheduler txs and add to hashset
        let scheduler_txs = self.pegout_scheduler.lock().await.tracked_pegout_request_ids();
        broadcasted_pegout_ids.extend(scheduler_txs);
        let pegouts_refs: Vec<&PegoutRequest> = pegouts
            .iter()
            .filter(|pegout| {
                if broadcasted_pegout_ids.contains(&pegout.id) {
                    error!(
                        "Received a pegout request for finalized or broadcasted id: L2 txid - {:?}, tx receipt log index - {:?}, bitcoin address - {:?}",
                        hex::encode(pegout.id.txid),
                        pegout.id.idx,
                        bitcoin::Address::from_script(&pegout.spk, self.btc_network)
                    );
                    return false;
                }
                true
            })
            .collect();

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

    /// Returns all finalized pegout ids
    async fn get_finalized_pegout_ids(
        &self,
        req: tonic::Request<rpc::GetFinalizedPegoutIdsRequest>,
    ) -> Result<tonic::Response<Self::GetFinalizedPegoutIdsStream>, tonic::Status> {
        self.validate_jwt(&req)?;
        let db = self.db.clone();
        let telemetry = self.telemetry.clone();
        let request = req.into_inner();

        let (tx, rx) = tokio::sync::mpsc::channel(request.chunk_size as usize);
        info!(
            "get_finalized_pegout_ids: Starting stream task (chunk_size={})...",
            request.chunk_size
        );
        tokio::spawn(async move {
            let stream = db.get_finalized_pegout_ids_stream(request.chunk_size as usize);
            pin_mut!(stream);
            trace!("get_finalized_pegout_ids stream task: Created DB stream.");

            while let Some(chunk_result) = stream.next().await {
                trace!(
                    "get_finalized_pegout_ids stream task: Received chunk from DB stream: {:?}",
                    chunk_result
                );
                match chunk_result {
                    Ok((pegout_ids, chunk_index, total_chunks)) => {
                        if let Some(telemetry) = telemetry.as_ref() {
                            telemetry.update_finalized_pegout_ids(pegout_ids.len() as i64);
                        }

                        let batch = rpc::GetFinalizedPegoutIdsResponse {
                            data: pegout_ids
                                .into_iter()
                                .map(|p| rpc::FinalizedPegout {
                                    id: p.id.as_bytes().to_vec(),
                                    botanix_block_height: p.block_number,
                                })
                                .collect(),
                            chunk_index,
                            total_chunks,
                            is_final: chunk_index + 1 == total_chunks,
                        };

                        // send the batch with retries
                        trace!("get_finalized_pegout_ids stream task: Sending chunk {}/{} with {} IDs to client.", chunk_index + 1, total_chunks, batch.data.len());
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
            trace!("get_finalized_pegout_ids stream task: DB stream finished.");
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
        let signing_session_id = parse_signing_session_id(&req.signing_session_id).to_status()?;

        // Check if we have already provided nonces for the current session
        let mut nonces_lock = self.frost_round1_nonces.lock().await;
        if nonces_lock.is_some() {
            if let Some(telemetry) = self.telemetry.as_ref() {
                telemetry.update_signing_error_metrics(
                    self.btc_network,
                    self.config.identifier,
                    Some(signing_session_id),
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
                    Some(signing_session_id),
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
        let pending_pegouts = self.db.get_pending_pegouts().to_status()?;
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

        if !self.is_coordinator() {
            // the coordinator will remove the pegout during finalize_signing
            self.db.reset_pending_pegouts(&[]).to_status()?;
            info!("[get_round2_signing_package] Pending pegouts removed and DB flushed.");
        }

        self.db.flush().to_status()?;

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
                match err_msg.as_str() {
                    msg if msg.contains("already in chain") => {
                        info!("Transaction already in chain, skipping");
                        Ok(None)
                    }
                    msg if msg.contains("bad-txns-inputs-missingorspent") => {
                        error!("Invalid input detected: {}", msg);
                        self.handle_invalid_inputs(&tx).to_status()?;
                        Err(CoordinatorError::FailedToBroadcastTx(err))
                    }
                    msg if msg.contains("dust") => {
                        error!("Dust output detected: {}", msg);
                        self.handle_dust_outputs(&tx, &psbt).to_status()?;
                        Err(CoordinatorError::FailedToBroadcastTx(err))
                    }
                    _ => {
                        error!("Failed to broadcast transaction: {}", err_msg);
                        error!("Failed tx: {:?}", tx);
                        Err(CoordinatorError::FailedToBroadcastTx(err))
                    }
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

        // First sync the pegout scheduler as this may add tracked pegouts back to the pending
        // pegouts list
        self.sync_pegout_scheduler(checkpoint).await.to_status()?;
        let tracked_txs = self.db.get_tracked_txs().to_status()?;

        // Select up to `UPPER_PEGOUT_BOUND` pegouts, sorted by age in ascending
        // order. Respectively, the oldest pegouts come first.
        let pending_pegouts = self.db.coord_pending_pegouts(UPPER_PEGOUT_BOUND).to_status()?;

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

        let psbt = coordinator::make_tx(
            outputs,
            fee_rate,
            change_script,
            &self.db,
            self.min_signers,
            tracked_txs,
            &self.config,
        )
        .await
        .to_status()?;

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

        info!("number of inputs = {}", psbt.unsigned_tx.input.len());
        info!("serialized unsigned tx = {}", bitcoin::consensus::encode::serialize_hex(&psbt.unsigned_tx));
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

        // NOTE: we do not make an existing aggregated key check at the start
        // here, since we still want to respond to the coordinator in case they
        // have not received our acknowledgment yet.

        let req = req.into_inner();
        let sender = deserialize_frost_peer_id(req.sender.clone()).to_status()?;
        let recipient = deserialize_frost_peer_id(req.recipient.clone()).to_status()?;

        // Decode the payload.
        let msg =
            ciborium::from_reader::<dkg::DkgMessage, _>(req.payload.as_slice()).to_status()?;

        let payload = dkg::DkgPayload { sender, recipient, msg };

        // Acquire the lock on the dkg machine.
        let mut l = self.dkg.lock().await;
        let Some(dkg) = l.as_mut() else {
            return Err(tonic::Status::internal("dkg not initialized"));
        };

        // Process the payload.
        dkg.machine.recv(payload).to_status()?;

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

                self.db.set_key_package(sec_key.clone()).to_status()?;
                self.db.set_pubkey_package(pub_key.clone()).to_status()?;
                self.db.flush().to_status()?;

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

    async fn recover_missing_utxos(
        &self,
        request: tonic::Request<RecoverMissingUtxosRequest>,
    ) -> Result<tonic::Response<RecoverMissingUtxosResponse>, tonic::Status> {
        // print the request
        info!("BtcServer::recover_missing_utxos: Request: {:?}", request);

        self.validate_jwt(&request)?;

        let total_requested = request.get_ref().utxos.len() as u64;
        info!("BtcServer::recover_missing_utxos: Total UTXOs requested: {}", total_requested);

        // Get the UTXO set from the db
        let db_utxos = self.db.get_all_utxos().to_status()?;
        if db_utxos.is_empty() {
            // Not returning an error as it's possible for the db utxos to be empty after a wallet
            // sweep
            warn!("BtcServer::recover_missing_utxos: No UTXOs found in the database.");
        }
        info!("BtcServer::recover_missing_utxos: Found {} UTXOs in the database.", db_utxos.len());

        let db_outpoints = db_utxos.iter().map(|u| u.outpoint).collect::<HashSet<_>>();

        // Ensure we have a key package
        let key_package = self
            .db
            .get_key_package()
            .to_status()?
            .ok_or(tonic::Status::internal("Missing key package"))?;

        let mut utxos_to_add = Vec::new();
        for req_utxo in request.into_inner().utxos {
            // convert the request outpoint to the database outpoint
            let req_outpoint = req_utxo.outpoint.as_ref().ok_or_else(|| {
                error!("BtcServer::recover_missing_utxos: UTXO has no outpoint");
                tonic::Status::invalid_argument("UTXO missing outpoint")
            })?;

            let outpoint = bitcoin::OutPoint::try_from(req_outpoint.clone()).map_err(|e| {
                error!("BtcServer::recover_missing_utxos: Invalid outpoint format: {}", e);
                tonic::Status::invalid_argument(format!("Invalid outpoint format: {}", e))
            })?;
            info!("BtcServer::recover_missing_utxos: converted bitcoin::OutPoint: {:?}", outpoint);

            // check if the utxo is already in the database
            if db_outpoints.contains(&outpoint) {
                warn!(
                    "BtcServer::recover_missing_utxos: UTXO {} is already in the database.",
                    outpoint
                );
                continue;
            }

            // verify that the utxo exists on chain
            let on_chain_utxo = self
                .bitcoind_client
                .get_tx_out(&outpoint.txid, outpoint.vout, None)
                .map_err(|e| {
                    error!(
                        "BtcServer::recover_missing_utxos: Failed to get tx out for input: {}: {}",
                        outpoint, e
                    );
                    tonic::Status::internal(format!(
                        "Failed to get tx out for input: {}: {}",
                        outpoint, e
                    ))
                })?;

            debug!("BtcServer::recover_missing_utxos: outpoint: {:?}", outpoint);
            debug!("BtcServer::recover_missing_utxos: on_chain_utxo: {:?}", on_chain_utxo);
            let Some(on_chain_utxo) = on_chain_utxo else {
                warn!(
                    "BtcServer::recover_missing_utxos: UTXO {} does not exist on chain, skipping.",
                    outpoint
                );
                continue;
            };

            // parse the eth address if it is not empty
            let eth_address: Option<[u8; 20]> = if req_utxo.eth_address.is_empty() {
                None
            } else {
                let parsed_eth_address = parse_eth_address(req_utxo.eth_address).map_err(|e| {
                    error!("BtcServer::recover_missing_utxos: Invalid ETH address format: {}", e);
                    tonic::Status::internal(format!("Invalid ETH address format: {}", e))
                })?;
                Some(parsed_eth_address)
            };

            // convert on chain utxo to database utxo
            let utxo = crate::database::Utxo {
                outpoint,
                output: TxOut {
                    value: on_chain_utxo.value,
                    script_pubkey: bitcoin::ScriptBuf::from_bytes(
                        on_chain_utxo.script_pub_key.hex.clone(),
                    ),
                },
                eth_address,
                version: 0,
            };

            // Generate the expected scriptPubKey for the eth address (if present) or the change
            // scriptPubKey otherwise
            let expected_script_pubkey: ScriptBuf;
            if let Some(eth_address) = eth_address {
                let agg_key = key_package.verifying_key();
                let tweaked_key = generate_tweaked_public_key(agg_key, &eth_address)
                    .map_err(|e| internal!("Failed to generate tweaked public key: {}", e))?;
                expected_script_pubkey = generate_taproot_scriptpubkey(&tweaked_key);
            } else {
                let secp_pk = key_package
                    .verifying_key()
                    .to_secp_pk()
                    .map_err(|e| internal!("Failed to generate tweaked public key: {}", e))?;
                expected_script_pubkey =
                    wallet::address::generate_taproot_change_scriptpubkey(&secp_pk);
            }

            // verify that the scriptPubKey matches the p2tr_script
            if expected_script_pubkey.to_bytes() != utxo.output.script_pubkey.to_bytes() {
                error!("BtcServer::recover_missing_utxos: UTXO {} does not match the tweaked scriptPubKey.", utxo.outpoint);
                continue;
            }

            // add the utxo to the list of utxos to be added
            info!("BtcServer::recover_missing_utxos: UTXO {} passed all validations, adding to recovery list", outpoint);
            utxos_to_add.push(utxo);
        }

        // add the utxos to the database
        let utxo_refs: Vec<&crate::database::Utxo> = utxos_to_add.iter().collect();
        info!("BtcServer::recover_missing_utxos: Storing {} missing UTXOs.", utxo_refs.len());
        self.db.store_utxos(&utxo_refs).to_status()?;
        self.db.update_utxo_merkle_root().to_status()?;
        self.db.flush().to_status()?;

        Ok(tonic::Response::new(RecoverMissingUtxosResponse {
            total_requested,
            total_recovered: utxos_to_add.len() as u64,
        }))
    }
}

impl<BitcoindClient: bitcoincore_rpc::RpcApi> App<BitcoindClient> {
    /// Handles invalid inputs in the transaction by:
    /// 1. Checking if the input's previous output exists in the database.
    /// 2. If it exists, checks if it's already spent.
    /// 3. If it is spent, removes it from the database.
    ///
    /// Returns `Ok(())` if all inputs are handled successfully, or an error if any operation fails.
    fn handle_invalid_inputs(&self, tx: &Transaction) -> Result<(), btcserverlib::database::Error> {
        for input in &tx.input {
            if let Some(_utxo) = self.db.get_utxo(input.previous_output)? {
                // Check on chain if the input is already spent
                let result = self
                    .bitcoind_client
                    .get_tx_out(&input.previous_output.txid, input.previous_output.vout, None)
                    .map_err(|e| {
                        error!("Failed to get tx out for input: {}: {}", input.previous_output, e);
                        btcserverlib::database::Error::BitcoindError(e)
                    })?;

                if result.is_none() {
                    // The input is already spent, remove it from the database
                    match self.db.remove_utxo(&input.previous_output) {
                        Ok(_) => {
                            info!("Removed spent input: {} from DB", input.previous_output);
                        }
                        Err(e) => {
                            error!(
                                "Failed to remove spent input: {} from DB: {}",
                                input.previous_output, e
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Removes from pending_pegouts any pegouts that correspond to dust outputs in the failed tx.
    fn handle_dust_outputs(
        &self,
        tx: &Transaction,
        psbt: &Psbt,
    ) -> Result<(), btcserverlib::database::Error> {
        let mut dust_pegout_ids: Vec<PegoutId> = Vec::new();
        for (vout, tx_out) in tx.output.iter().enumerate() {
            if tx_out.value >= tx_out.script_pubkey.minimal_non_dust() {
                info!(
                    "Output is not a dust output, skipping: vout {}, value {}",
                    vout, tx_out.value
                );
                continue;
            }
            let pegout_id: PegoutId = match psbt.outputs.get(vout).and_then(|o| o.pegout_id()) {
                Some(bytes) => match PegoutId::from_bytes(&bytes) {
                    Ok(id) => id,
                    Err(()) => {
                        error!("Failed to parse pegout id for dust output with vout {} and value {}, skipping", vout, tx_out.value);
                        continue;
                    }
                },
                None => {
                    info!("Output is not a pegout output (may be a change output) with skipping: vout {}, value {}", vout, tx_out.value);
                    continue;
                }
            };
            dust_pegout_ids.push(pegout_id);
        }
        if dust_pegout_ids.is_empty() {
            error!("No dust pegouts detected in the transaction");
            return Err(btcserverlib::database::Error::NoDustPegoutsIdentified);
        }

        info!(
            "Removing {} dust pegouts from pending_pegouts: {:?}",
            dust_pegout_ids.len(),
            dust_pegout_ids
        );
        self.db.remove_pending_pegout(&dust_pegout_ids)?;
        self.db.flush()?;
        Ok(())
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
    use bitcoin::{secp256k1, OutPoint, Script, Txid};
    use btcserverlib::{dkg::DkgMessage, wallet::address::generate_taproot_change_scriptpubkey};
    use frost_secp256k1_tr::keys::dkg::round1;
    use rand::{thread_rng, Rng};
    use std::{str::FromStr, vec};
    use tempfile::TempDir;
    use url::Url;

    use super::*;
    use btcserverlib::{
        frost_id,
        test_utils::{
            create_random_pegout_id, create_tx, random_compute_txid, random_p2tr_keyspend_script,
            random_p2wpkh_script, trusted_dealer_setup, MockBitcoind,
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
            db: temp_db.keep(),
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
            excluded_eth_addresses: vec![],
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
    async fn test_finalized_pegout_ids_streaming_chunksize_gt_chunks() {
        let app = setup().await;
        let num_txs = 52;
        let mut finalized_pegout_ids = vec![];
        let mut rng = thread_rng();
        for i in 0..num_txs {
            let pegout_id = PegoutId::new(rng.gen::<[u8; 32]>(), i as u32);
            let finalized_pegout =
                btcserverlib::database::FinalizedPegout { id: pegout_id, block_number: 100 };
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
            let finalized_pegout =
                btcserverlib::database::FinalizedPegout { id: pegout_id, block_number: 100 };
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

    /// Helper function to create test input from OutPoint
    fn create_test_input(outpoint: OutPoint) -> bitcoin::TxIn {
        bitcoin::TxIn {
            previous_output: outpoint,
            script_sig: ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::default(),
        }
    }

    /// Helper function to create test transaction with given inputs and outputs
    fn create_test_transaction(inputs: Vec<OutPoint>) -> Transaction {
        Transaction {
            version: bitcoin::transaction::Version(2),
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: inputs.into_iter().map(create_test_input).collect(),
            output: vec![bitcoin::TxOut {
                value: Amount::from_sat(1000),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }

    #[tokio::test]
    async fn test_handle_invalid_inputs_removes_spent_utxos() {
        let app = setup().await;
        let mut rng = thread_rng();

        // Generate 3 different outpoints for testing
        let input_1 = OutPoint::new(Txid::from_slice(&rng.gen::<[u8; 32]>()).unwrap(), 0);
        let input_2 = OutPoint::new(Txid::from_slice(&rng.gen::<[u8; 32]>()).unwrap(), 1);
        let input_3 = OutPoint::new(Txid::from_slice(&rng.gen::<[u8; 32]>()).unwrap(), 0);

        // Create transaction with these inputs
        let tx = create_test_transaction(vec![input_1, input_2, input_3]);

        // Set up mock responses: input_1 is spent, input_2 and input_3 are unspent
        app.bitcoind_client.remove_utxo(input_1);
        app.bitcoind_client.add_utxo(input_2, Amount::from_sat(5000), ScriptBuf::new());
        app.bitcoind_client.add_utxo(input_3, Amount::from_sat(3000), ScriptBuf::new());

        // Add all 3 UTXOs to the database initially
        let utxos = vec![
            database::Utxo::new(
                input_1,
                bitcoin::TxOut { value: Amount::from_sat(2000), script_pubkey: ScriptBuf::new() },
                None,
                None,
            ),
            database::Utxo::new(
                input_2,
                bitcoin::TxOut { value: Amount::from_sat(5000), script_pubkey: ScriptBuf::new() },
                None,
                None,
            ),
            database::Utxo::new(
                input_3,
                bitcoin::TxOut { value: Amount::from_sat(3000), script_pubkey: ScriptBuf::new() },
                None,
                None,
            ),
        ];
        let utxo_refs: Vec<&database::Utxo> = utxos.iter().collect();
        app.db.store_utxos(&utxo_refs).expect("Failed to store UTXOs");

        // Verify all UTXOs are in database initially
        assert!(app.db.get_utxo(input_1).unwrap().is_some());
        assert!(app.db.get_utxo(input_2).unwrap().is_some());
        assert!(app.db.get_utxo(input_3).unwrap().is_some());

        // Call handle_invalid_inputs
        let result = app.handle_invalid_inputs(&tx);
        assert!(result.is_ok(), "handle_invalid_inputs should succeed");

        // Verify that spent UTXOs (input_1 and input_3) are removed from database
        assert!(
            app.db.get_utxo(input_1).unwrap().is_none(),
            "Spent UTXO input_1 should be removed"
        );
        assert!(app.db.get_utxo(input_2).unwrap().is_some(), "Unspent UTXO input_2 should remain");
        assert!(app.db.get_utxo(input_3).unwrap().is_some(), "Unspent UTXO input_3 should remain");
    }

    #[tokio::test]
    async fn test_handle_invalid_inputs_ignores_missing_utxos() {
        let app = setup().await;

        // Create transaction with input that doesn't exist in database
        let missing_input =
            OutPoint::new(Txid::from_slice(&thread_rng().gen::<[u8; 32]>()).unwrap(), 0);
        let tx = create_test_transaction(vec![missing_input]);

        // Verify UTXO doesn't exist in database
        assert!(app.db.get_utxo(missing_input).unwrap().is_none());

        // Call handle_invalid_inputs - should not error even though UTXO doesn't exist
        let result = app.handle_invalid_inputs(&tx);
        assert!(result.is_ok(), "handle_invalid_inputs should succeed even with missing UTXOs");
    }

    #[tokio::test]
    async fn test_handle_invalid_inputs_handles_all_unspent() {
        let app = setup().await;
        let mut rng = thread_rng();

        // Create test outpoints
        let input_1 = OutPoint::new(Txid::from_slice(&rng.gen::<[u8; 32]>()).unwrap(), 0);
        let input_2 = OutPoint::new(Txid::from_slice(&rng.gen::<[u8; 32]>()).unwrap(), 1);

        let tx = create_test_transaction(vec![input_1, input_2]);

        // Set both inputs as unspent
        app.bitcoind_client.add_utxo(input_1, Amount::from_sat(2000), ScriptBuf::new()); // Changed
        app.bitcoind_client.add_utxo(input_2, Amount::from_sat(3000), ScriptBuf::new()); // Changed

        // Add both UTXOs to the database
        let utxos = vec![
            database::Utxo::new(
                input_1,
                bitcoin::TxOut { value: Amount::from_sat(2000), script_pubkey: ScriptBuf::new() },
                None,
                None,
            ),
            database::Utxo::new(
                input_2,
                bitcoin::TxOut { value: Amount::from_sat(3000), script_pubkey: ScriptBuf::new() },
                None,
                None,
            ),
        ];
        let utxo_refs: Vec<&database::Utxo> = utxos.iter().collect();
        app.db.store_utxos(&utxo_refs).expect("Failed to store UTXOs");

        // Verify both UTXOs are in database initially
        assert!(app.db.get_utxo(input_1).unwrap().is_some());
        assert!(app.db.get_utxo(input_2).unwrap().is_some());

        // Call handle_invalid_inputs
        let result = app.handle_invalid_inputs(&tx);
        assert!(result.is_ok(), "handle_invalid_inputs should succeed");

        // Verify that both UTXOs remain in database since they're unspent
        assert!(app.db.get_utxo(input_1).unwrap().is_some(), "Unspent UTXO input_1 should remain");
        assert!(app.db.get_utxo(input_2).unwrap().is_some(), "Unspent UTXO input_2 should remain");
    }

    // Helper function to set up app with key package
    async fn setup_app_with_keys() -> (App<MockBitcoind>, frost::keys::KeyPackage) {
        let app = setup().await;
        let (shares, pk_package) = trusted_dealer_setup(app.min_signers, app.max_signers);
        let key_package = frost::keys::KeyPackage::try_from(shares[&app.identifier].clone())
            .expect("valid key package");
        app.db.set_pubkey_package(pk_package).expect("set public key package");
        app.db.set_key_package(key_package.clone()).expect("set key package");
        (app, key_package)
    }

    fn create_random_outpoint(rng: &mut impl Rng) -> OutPoint {
        OutPoint::new(Txid::from_slice(&rng.gen::<[u8; 32]>()).unwrap(), 0)
    }

    fn create_utxo(
        rng: &mut impl Rng,
        amount: u64,
        agg_key: &frost::VerifyingKey,
        eth_address: Option<[u8; 20]>,
    ) -> (OutPoint, Amount, ScriptBuf) {
        let amount = Amount::from_sat(amount);
        let script_pubkey = if let Some(eth_address) = eth_address {
            let tweaked_key =
                generate_tweaked_public_key(agg_key, &eth_address).expect("valid tweaked key");
            generate_taproot_scriptpubkey(&tweaked_key)
        } else {
            let secp_pk = agg_key.to_secp_pk().expect("valid secp key");
            generate_taproot_change_scriptpubkey(&secp_pk)
        };
        let outpoint = create_random_outpoint(rng);

        (outpoint, amount, script_pubkey)
    }

    fn add_utxo_to_db(
        app: &App<MockBitcoind>,
        outpoint: OutPoint,
        amount: Amount,
        script_pubkey: ScriptBuf,
        eth_address: Option<[u8; 20]>,
    ) {
        let utxo = database::Utxo::new(
            outpoint,
            bitcoin::TxOut { value: amount, script_pubkey: script_pubkey.clone() },
            eth_address,
            None,
        );

        let utxo_refs: Vec<&database::Utxo> = vec![&utxo];
        app.db.store_utxos(&utxo_refs).expect("Failed to store UTXO");
    }

    #[tokio::test]
    async fn test_recover_missing_utxos_success() {
        let (app, key_package) = setup_app_with_keys().await;
        let agg_key = key_package.verifying_key();
        let mut rng = thread_rng();

        // add dummy utxo to db to prevent 'no utxo in db' error
        let (dummy_outpoint, dummy_amount, dummy_script_pubkey) =
            create_utxo(&mut rng, 1000, agg_key, None);
        add_utxo_to_db(&app, dummy_outpoint, dummy_amount, dummy_script_pubkey, None);

        // Onchain UTXO 1: With eth_address (pegin UTXO)
        let eth_address = [1u8; 20];
        let (outpoint1, utxo1_amount, pegin_script_pubkey) =
            create_utxo(&mut rng, 100000, agg_key, Some(eth_address));
        app.bitcoind_client.add_utxo(outpoint1, utxo1_amount, pegin_script_pubkey.clone());

        // Onchain UTXO 2: change UTXO
        let (outpoint2, utxo2_amount, change_script_pubkey) =
            create_utxo(&mut rng, 50000, agg_key, None);
        app.bitcoind_client.add_utxo(outpoint2, utxo2_amount, change_script_pubkey.clone());

        // Create request utxos
        let utxo_with_eth = rpc::UtxoToRecover {
            outpoint: Some(rpc::OutPoint::from(outpoint1)),
            eth_address: hex::encode(eth_address),
        };

        let utxo_without_eth = rpc::UtxoToRecover {
            outpoint: Some(rpc::OutPoint::from(outpoint2)),
            eth_address: String::new(), // Empty for change UTXO
        };

        let request = tonic::Request::new(RecoverMissingUtxosRequest {
            utxos: vec![utxo_with_eth, utxo_without_eth],
        });

        let response = app.recover_missing_utxos(request).await.expect("successful recovery");
        let inner = response.into_inner();

        // Verify results
        assert_eq!(inner.total_requested, 2);
        assert_eq!(inner.total_recovered, 2);

        // Verify UTXOs were stored in database
        let stored_utxo1 = app.db.get_utxo(outpoint1).unwrap().unwrap();
        assert_eq!(stored_utxo1.output.value, utxo1_amount);
        assert_eq!(stored_utxo1.output.script_pubkey, pegin_script_pubkey,);

        let stored_utxo2 = app.db.get_utxo(outpoint2).unwrap().unwrap();
        assert_eq!(stored_utxo2.output.value, utxo2_amount,);
        assert_eq!(stored_utxo2.output.script_pubkey, change_script_pubkey,);
    }

    #[tokio::test]
    async fn test_recover_missing_utxos_bad_requests() {
        let (app, key_package) = setup_app_with_keys().await;
        let agg_key = key_package.verifying_key();
        let mut rng = thread_rng();

        // add existing utxo to db
        let (existing_outpoint, existing_amount, existing_script_pubkey) =
            create_utxo(&mut rng, 1000, agg_key, None);
        add_utxo_to_db(&app, existing_outpoint, existing_amount, existing_script_pubkey, None);

        // add these utxos to bitcoind
        // Onchain UTXO 1: With eth_address (pegin UTXO)
        let eth_address = [1u8; 20];
        let (outpoint1, utxo1_amount, pegin_script_pubkey) =
            create_utxo(&mut rng, 100000, agg_key, Some(eth_address));
        app.bitcoind_client.add_utxo(outpoint1, utxo1_amount, pegin_script_pubkey.clone());

        // Onchain UTXO 2: change UTXO
        let not_change_script_pubkey = ScriptBuf::new();
        let (outpoint2, utxo2_amount, _) = create_utxo(&mut rng, 50000, agg_key, None);
        app.bitcoind_client.add_utxo(outpoint2, utxo2_amount, not_change_script_pubkey);

        // (not onchain) UTXO 3: not found by bitcoind
        let (outpoint3, _, _) = create_utxo(&mut rng, 50000, agg_key, None);

        // Case 1 - utxo exists in db
        let existing_utxo = rpc::UtxoToRecover {
            outpoint: Some(rpc::OutPoint::from(existing_outpoint)),
            eth_address: String::new(),
        };

        // Case 2 - wrong eth address
        let wrong_eth_address = [2u8; 20];
        let utxo_wrong_eth_address = rpc::UtxoToRecover {
            outpoint: Some(rpc::OutPoint::from(outpoint1)),
            eth_address: hex::encode(wrong_eth_address),
        };

        // Case 3 - utxo does not match change script pubkey
        let utxo_not_change_script = rpc::UtxoToRecover {
            outpoint: Some(rpc::OutPoint::from(outpoint2)),
            eth_address: String::new(),
        };

        // Case 4 - utxo is not found by bitcoind
        let utxo_not_found = rpc::UtxoToRecover {
            outpoint: Some(rpc::OutPoint::from(outpoint3)),
            eth_address: String::new(),
        };

        let request = tonic::Request::new(RecoverMissingUtxosRequest {
            utxos: vec![
                existing_utxo,
                utxo_wrong_eth_address,
                utxo_not_change_script,
                utxo_not_found,
            ],
        });

        let response = app.recover_missing_utxos(request).await.expect("successful recovery");
        let inner = response.into_inner();

        // Verify results
        assert_eq!(inner.total_requested, 4);
        assert_eq!(inner.total_recovered, 0);

        // Verify no bad utxos were added to db
        let utxos = app.db.get_all_utxos().unwrap();
        assert_eq!(utxos.len(), 1);
        assert_eq!(utxos[0].outpoint, existing_outpoint);
    }

    #[tokio::test]
    async fn test_handle_dust_outputs_single_dust_pegout() {
        use crate::wallet::psbt::PsbtOutputExt;
        use bitcoin::{
            absolute::LockTime, psbt::Psbt, Amount, ScriptBuf, Sequence, Transaction, TxIn, TxOut,
        };

        let app = setup().await;

        // Create a transaction with a single dust output (< 330 sats)
        let dust_value = Amount::from_sat(200); // Below 330 sat threshold
        let pegout_id = create_random_pegout_id();

        // Store the pegout in the database
        let pegout_request = btcserverlib::pegout_scheduler::PegoutRequest {
            id: pegout_id,
            value: dust_value,
            spk: random_p2wpkh_script(),
            botanix_height: 0,
        };
        app.db.store_pending_pegout(&pegout_request).unwrap();

        // Verify pegout is in the database
        assert_eq!(app.db.get_pending_pegouts().unwrap().len(), 1);

        // Create output with dust value
        let output = TxOut { value: dust_value, script_pubkey: random_p2wpkh_script() };

        // Create transaction manually
        let tx = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(random_compute_txid(), 0),
                sequence: Sequence::MAX,
                script_sig: ScriptBuf::new(),
                witness: Default::default(),
            }],
            output: vec![output.clone()],
        };

        // Create PSBT and add pegout ID
        let mut psbt = Psbt::from_unsigned_tx(tx.clone()).expect("tx is unsigned");
        psbt.inputs[0].witness_utxo =
            Some(TxOut { value: Amount::from_sat(10000), script_pubkey: ScriptBuf::new() });
        psbt.outputs[0].set_pegout_id(pegout_id.as_bytes());

        // Call handle_dust_outputs
        let result = app.handle_dust_outputs(&tx, &psbt);

        // Should succeed
        assert!(result.is_ok());

        // Verify the dust pegout was removed from pending_pegouts
        let remaining_pegouts = app.db.get_pending_pegouts().unwrap();
        assert_eq!(remaining_pegouts.len(), 0);
    }

    #[tokio::test]
    async fn test_handle_dust_outputs_multiple_outputs_some_dust() {
        use crate::wallet::psbt::PsbtOutputExt;
        use bitcoin::{
            absolute::LockTime, psbt::Psbt, Amount, ScriptBuf, Sequence, Transaction, TxIn, TxOut,
        };

        let app = setup().await;

        let dust_value = Amount::from_sat(250); // Below 330 sat threshold
        let non_dust_value = Amount::from_sat(1000); // Above 330 sat threshold

        let dust_pegout_id = create_random_pegout_id();
        let non_dust_pegout_id = create_random_pegout_id();

        // Store both pegouts in the database
        let dust_pegout_request = btcserverlib::pegout_scheduler::PegoutRequest {
            id: dust_pegout_id,
            value: dust_value,
            spk: random_p2wpkh_script(),
            botanix_height: 0,
        };
        app.db.store_pending_pegout(&dust_pegout_request).unwrap();

        let non_dust_pegout_request = btcserverlib::pegout_scheduler::PegoutRequest {
            id: non_dust_pegout_id,
            value: non_dust_value,
            spk: random_p2wpkh_script(),
            botanix_height: 0,
        };
        app.db.store_pending_pegout(&non_dust_pegout_request).unwrap();

        // Verify both pegouts are in the database
        assert_eq!(app.db.get_pending_pegouts().unwrap().len(), 2);

        // Create outputs - one dust, one non-dust
        let dust_output = TxOut { value: dust_value, script_pubkey: random_p2wpkh_script() };

        let non_dust_output =
            TxOut { value: non_dust_value, script_pubkey: random_p2wpkh_script() };

        // Create transaction manually
        let tx = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(random_compute_txid(), 0),
                sequence: Sequence::MAX,
                script_sig: ScriptBuf::new(),
                witness: Default::default(),
            }],
            output: vec![dust_output.clone(), non_dust_output.clone()],
        };

        // Create PSBT and add pegout IDs
        let mut psbt = Psbt::from_unsigned_tx(tx.clone()).expect("tx is unsigned");
        psbt.inputs[0].witness_utxo =
            Some(TxOut { value: Amount::from_sat(20000), script_pubkey: ScriptBuf::new() });
        psbt.outputs[0].set_pegout_id(dust_pegout_id.as_bytes());
        psbt.outputs[1].set_pegout_id(non_dust_pegout_id.as_bytes());

        // Call handle_dust_outputs
        let result = app.handle_dust_outputs(&tx, &psbt);

        // Should succeed
        assert!(result.is_ok());

        // Verify only the dust pegout was removed
        let remaining_pegouts = app.db.get_pending_pegouts().unwrap();
        assert_eq!(remaining_pegouts.len(), 1);
        assert_eq!(remaining_pegouts[0].id, non_dust_pegout_id);
    }

    #[tokio::test]
    async fn test_handle_dust_outputs_dust_change_output() {
        use crate::wallet::psbt::PsbtOutputExt;
        use bitcoin::{
            absolute::LockTime, psbt::Psbt, Amount, ScriptBuf, Sequence, Transaction, TxIn, TxOut,
        };

        let app = setup().await;

        let pegout_value = Amount::from_sat(1000); // Above dust threshold
        let change_value = Amount::from_sat(200); // Below dust threshold - but it's change

        let pegout_id = create_random_pegout_id();

        // Store the pegout in the database
        let pegout_request = btcserverlib::pegout_scheduler::PegoutRequest {
            id: pegout_id,
            value: pegout_value,
            spk: random_p2wpkh_script(),
            botanix_height: 0,
        };
        app.db.store_pending_pegout(&pegout_request).unwrap();

        // Verify pegout is in the database
        assert_eq!(app.db.get_pending_pegouts().unwrap().len(), 1);

        // Create pegout output (non-dust)
        let pegout_output = TxOut { value: pegout_value, script_pubkey: random_p2wpkh_script() };

        // Create change output (dust, but has no pegout ID)
        let change_output = TxOut { value: change_value, script_pubkey: random_p2wpkh_script() };

        // Create transaction manually
        let tx = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(random_compute_txid(), 0),
                sequence: Sequence::MAX,
                script_sig: ScriptBuf::new(),
                witness: Default::default(),
            }],
            output: vec![pegout_output.clone(), change_output.clone()],
        };

        // Create PSBT and add pegout ID only to first output (not to change)
        let mut psbt = Psbt::from_unsigned_tx(tx.clone()).expect("tx is unsigned");
        psbt.inputs[0].witness_utxo =
            Some(TxOut { value: Amount::from_sat(20000), script_pubkey: ScriptBuf::new() });
        psbt.outputs[0].set_pegout_id(pegout_id.as_bytes());
        // Note: psbt.outputs[1] (change) has no pegout_id set

        // Call handle_dust_outputs
        let result = app.handle_dust_outputs(&tx, &psbt);

        // Should fail because no dust pegouts were found (change doesn't have pegout ID)
        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("NoDustPegoutsIdentified"));

        // Verify the pegout was NOT removed (because it's not dust)
        let remaining_pegouts = app.db.get_pending_pegouts().unwrap();
        assert_eq!(remaining_pegouts.len(), 1);
        assert_eq!(remaining_pegouts[0].id, pegout_id);
    }
}
