use std::{path::PathBuf, str::FromStr};

#[macro_use]
extern crate log;

use bitcoin::{
    base64::{self, Engine},
    psbt::Psbt,
    Amount, FeeRate, OutPoint, TxOut,
};
use btcserverlib::{
    badarg, database,
    util::parse_eth_address,
    wallet::{psbt::PsbtInputExt, util::calculate_signed_tx_weight},
};
use clap::Parser;
use frost_secp256k1_tr as frost;
use peginrecoverylib::database as recovery_db;
use rand::{thread_rng, RngCore};

use peginrecoverylib::rpc::pegin_recovery::{
    pegin_recovery_service_server::{PeginRecoveryService, PeginRecoveryServiceServer},
    Empty, ImportKeyShareRequest, RecoverPeginRequest, RecoverPeginResponse, FILE_DESCRIPTOR_SET,
};

use std::net::SocketAddr;
use tonic::{transport::Server, Request, Response, Status};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_PORT: u16 = 50052;

#[derive(Parser)]
#[command(name = "pegin-recovery")]
#[command(version = VERSION)]
#[command(about = "Pegin Recovery Service for FROST threshold signatures")]
struct Args {
    /// Path to the database
    #[arg(long, env = "PEGIN_RECOVERY_DB_PATH", default_value = "./pegin-recovery.db")]
    db: PathBuf,

    /// gRPC server port
    #[arg(long, env = "PEGIN_RECOVERY_PORT", default_value_t = DEFAULT_PORT)]
    port: u16,
}

const DUMMY_IDENTIFIER_SIZE: usize = 33;
const SIGNING_SESSION_ID_SIZE: usize = 32;

struct SigningPackage {
    psbt_base64: String,
    #[allow(dead_code)]
    identifier_hex: String,
    #[allow(dead_code)]
    signing_session_id_hex: String,
}

fn parse_and_validate_address(
    address_str: &str,
    testnet: bool,
) -> Result<bitcoin::Address, tonic::Status> {
    let network = if testnet { bitcoin::Network::Testnet } else { bitcoin::Network::Bitcoin };

    bitcoin::Address::from_str(address_str)
        .map_err(|e| badarg!("invalid address: {}", e))?
        .require_network(network)
        .map_err(|e| badarg!("address network error: {}", e))
}

#[allow(dead_code)]
fn validate_psbt_fee_sanity(psbt: &Psbt) -> anyhow::Result<()> {
    let fee = psbt.fee().map_err(|e| badarg!("Failed to calculate PSBT fee: {}", e))?;

    let total_outputs_amount =
        psbt.unsigned_tx.output.iter().fold(Amount::ZERO, |total, output| {
            total.checked_add(output.value).unwrap_or_default()
        });

    if fee > total_outputs_amount {
        return Err(badarg!(
            "Fee ({}) cannot be greater than total output value ({})",
            fee,
            total_outputs_amount
        )
        .into());
    }

    Ok(())
}

fn calculate_fee(
    utxos: &[database::Utxo],
    script_pubkey: &bitcoin::ScriptBuf,
    fee_rate: FeeRate,
) -> Result<Amount, tonic::Status> {
    let psbt = create_recovery_psbt(utxos.to_vec(), &script_pubkey, Amount::from_sat(0));
    let total_weight = calculate_signed_tx_weight(&psbt)
        .map_err(|e| badarg!("Failed to calculate signed tx weight: {}", e))?;
    let absolute_fee = fee_rate.fee_wu(total_weight).ok_or(badarg!("Fee rate overflow"))?;
    Ok(absolute_fee)
}

// Based on wallet::psbt::create_psbt but with a single output and no pegout id
pub(crate) fn create_recovery_psbt(
    inputs: Vec<database::Utxo>,
    script_pubkey: &bitcoin::ScriptBuf,
    value: Amount,
) -> Psbt {
    let output = TxOut { value, script_pubkey: script_pubkey.clone() };

    let tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::locktime::absolute::LockTime::ZERO,
        input: inputs
            .iter()
            .map(|u| bitcoin::TxIn {
                previous_output: u.outpoint,
                sequence: bitcoin::Sequence::ENABLE_RBF_NO_LOCKTIME,
                script_sig: bitcoin::ScriptBuf::new(),
                witness: Default::default(),
            })
            .collect(),
        output: vec![output],
    };

    // Create PSBT
    // add input meta
    let mut psbt = Psbt::from_unsigned_tx(tx).expect("tx is unsigned");
    for (psbt_input, utxo) in psbt.inputs.iter_mut().zip(inputs.iter()) {
        psbt_input.witness_utxo = Some(utxo.output.clone());
        if let Some(eth_addr) = utxo.eth_address {
            psbt_input.set_eth_address(eth_addr);
        }
        psbt_input.add_version_to_psbt(utxo.version as u32);
    }

    psbt
}

fn create_round1_signing_package(psbt: Psbt) -> Result<SigningPackage, tonic::Status> {
    // create random signing session id
    let mut signing_session_id = [0u8; SIGNING_SESSION_ID_SIZE];
    rand::thread_rng().fill_bytes(&mut signing_session_id);

    // create a dummy identifier (since it doesn't matter for this use case)
    let dummy_identifier = vec![1u8; DUMMY_IDENTIFIER_SIZE]; // 33 bytes for a compressed public key

    // create the serializable SigningPackage structure
    let signing_package = SigningPackage {
        psbt_base64: base64::prelude::BASE64_STANDARD.encode(psbt.serialize()),
        identifier_hex: hex::encode(&dummy_identifier),
        signing_session_id_hex: hex::encode(signing_session_id),
    };

    Ok(signing_package)
}

#[allow(dead_code)]
fn round1_signing_package(
    psbt: &mut Psbt,
    key_package: &frost::keys::KeyPackage,
    my_identifier: frost::Identifier,
) -> anyhow::Result<Vec<(frost::round1::SigningNonces, frost::round1::SigningCommitments)>> {
    // Basic PSBT sanity checks (but skip btc-server specific validations)
    if psbt.inputs.is_empty() {
        return Err(anyhow::anyhow!("PSBT must have at least one input"));
    }

    if psbt.outputs.is_empty() {
        return Err(anyhow::anyhow!("PSBT must have at least one output"));
    }

    // Basic fee sanity check
    validate_psbt_fee_sanity(psbt)?;

    // Core FROST logic (copied from signer::get_round1_signing_package)
    let num_inputs = psbt.inputs.len();
    let secret = key_package.signing_share();
    let mut nonces = vec![];
    let mut rng = thread_rng();

    // Generate nonces and commitments for each input
    // Order is important - each nonce pair corresponds to a transaction input
    for i in 0..num_inputs {
        let nonce_pkg = frost::round1::commit(secret, &mut rng);
        psbt.inputs[i].set_signing_commitment(my_identifier, &nonce_pkg.1);
        nonces.push(nonce_pkg);
    }

    Ok(nonces)
}

/// Main service implementation
#[derive(Debug, Clone)]
pub struct PeginRecoveryServiceImpl {
    db: recovery_db::Db,
}

impl PeginRecoveryServiceImpl {
    pub fn new(db: recovery_db::Db) -> Self {
        Self { db }
    }
}

#[tonic::async_trait]
impl PeginRecoveryService for PeginRecoveryServiceImpl {
    async fn health_check(&self, _request: Request<Empty>) -> Result<Response<Empty>, Status> {
        info!("Health check requested");
        Ok(Response::new(Empty {}))
    }

    async fn import_key_share(
        &self,
        request: Request<ImportKeyShareRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        info!("ImportKeyShare requested - multisig_id: {} bytes", req.multisig_id.len());

        // Deserialize the FROST identifier
        let node_identifier = frost::Identifier::deserialize(
            req.node_identifier
                .as_slice()
                .try_into()
                .map_err(|_| Status::invalid_argument("node_identifier must be 32 bytes"))?,
        )
        .map_err(|_| Status::invalid_argument("Invalid FROST identifier"))?;

        // Convert the protobuf ExportedKeyPackage to the Rust type
        let export_proto =
            req.export.ok_or_else(|| Status::invalid_argument("export is required"))?;
        let export = btcserverlib::database::ExportedKeyPackage {
            version: export_proto.version as u16,
            iv: export_proto
                .iv
                .as_slice()
                .try_into()
                .map_err(|_| Status::invalid_argument("iv must be 12 bytes"))?,
            enc_key_package: export_proto.enc_key_package,
            enc_pk_package: export_proto.enc_pk_package,
        };

        // Import the key
        self.db
            .import_from_btc_server(
                &req.multisig_id,
                node_identifier,
                zeroize::Zeroizing::new(req.passphrase),
                export,
            )
            .map_err(|e| Status::internal(format!("Failed to import key: {}", e)))?;

        info!("Successfully imported key share for node_identifier: {:?}", node_identifier);
        Ok(Response::new(Empty {}))
    }

    async fn recover_pegin(
        &self,
        request: Request<RecoverPeginRequest>,
    ) -> Result<Response<RecoverPeginResponse>, Status> {
        let req = request.into_inner();
        info!(
            "RecoverPegin requested - destination: {}, txid: {}, vout: {}",
            req.destination, req.txid, req.vout
        );

        // TODO: Implement pegin recovery logic
        // - do the signing rounds
        // - broadcast the transaction
        // - return the signed transaction and its txid

        let testnet = true; // TODO: get from config

        // parse and validate the request
        let destination = parse_and_validate_address(&req.destination, testnet)
            .map_err(|e| badarg!("Invalid destination: {}", e))?;
        let script_pubkey = destination.script_pubkey();

        let txid = req.txid.parse::<bitcoin::Txid>().map_err(|e| badarg!("Invalid txid: {}", e))?;
        let vout = req.vout;
        let eth_address = parse_eth_address(req.eth_address)
            .map_err(|e| badarg!("Invalid eth address: {}", e))?;

        // TODO: validate utxo is on chain and matches
        let input_amount = Amount::from_sat(1000); // TODO: get from on-chain validation

        // create the utxo
        let utxo = database::Utxo {
            outpoint: OutPoint::new(txid, vout),
            output: TxOut { value: input_amount, script_pubkey: script_pubkey.clone() },
            eth_address: Some(eth_address),
            version: 1,
        };
        let utxos = vec![utxo];
        // calculate the fee and subtract from the output value
        let fee_rate_sat_per_vbyte = 5; // TODO: get from config
        let fee_rate: FeeRate = FeeRate::from_sat_per_vb(fee_rate_sat_per_vbyte)
            .ok_or(badarg!("Invalid fee rate: {}", fee_rate_sat_per_vbyte))?;
        let absolute_fee = calculate_fee(&utxos, &script_pubkey, fee_rate)
            .map_err(|e| badarg!("Invalid fee: {}", e))?;
        let output_value =
            input_amount.checked_sub(absolute_fee).ok_or(badarg!("output value underflow"))?;

        // create the psbt
        let psbt = create_recovery_psbt(utxos, &script_pubkey, output_value);

        let signing_package = create_round1_signing_package(psbt.clone())?;

        // TODO:
        // for each key share, do round 1 signing
        // create round 2 signing package
        // for each key share, do round 2 signing
        // aggregate the signatures
        // broadcast the transaction

        Ok(Response::new(RecoverPeginResponse {
            tx: signing_package.psbt_base64,
            txid: txid.to_string(),
        }))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logger
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .filter_module("pegin_recovery", log::LevelFilter::Debug)
        .init();

    // Parse command line arguments
    let args = Args::parse();

    info!("Starting Pegin Recovery Service v{}", VERSION);

    // Open database
    let db = recovery_db::Db::open(&args.db)
        .map_err(|e| anyhow::anyhow!("Failed to open database at {:?}: {}", args.db, e))?;
    info!("Database opened at: {:?}", args.db);

    // Configure service address
    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    info!("gRPC server listening on {}", addr);

    // Create service
    let service = PeginRecoveryServiceImpl::new(db);
    let svc = PeginRecoveryServiceServer::new(service);

    // Configure reflection (for grpcurl and similar tools)
    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(FILE_DESCRIPTOR_SET)
        .build_v1()?;

    // Start server
    Server::builder().add_service(svc).add_service(reflection_service).serve(addr).await?;

    Ok(())
}
