use std::{path::PathBuf, str::FromStr, sync::Arc};

#[macro_use]
extern crate log;

use bitcoin::{psbt::Psbt, Amount, FeeRate, OutPoint, ScriptBuf, TxOut};
use bitcoincore_rpc::RpcApi;
use btcserverlib::{
    badarg, database,
    util::parse_eth_address,
    wallet::{
        address::{generate_taproot_scriptpubkey, generate_tweaked_public_key},
        psbt::{PsbtExt, PsbtInputExt},
        util::calculate_signed_tx_weight,
    },
};
use clap::Parser;
use frost_secp256k1_tr::{
    self as frost,
    keys::Tweak,
    round1::{SigningCommitments, SigningNonces},
    SigningParameters,
};
use miniscript::psbt::PsbtExt as MiniscriptPsbtExt;
use peginrecoverylib::database as recovery_db;
use rand::thread_rng;

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

    /// Bitcoin RPC URL
    #[arg(long, env = "BITCOIN_RPC_URL", default_value = "http://localhost:18443")]
    bitcoin_rpc_url: String,

    /// Bitcoin RPC username
    #[arg(long, env = "BITCOIN_RPC_USER", default_value = "user")]
    bitcoin_rpc_user: String,

    /// Bitcoin RPC password
    #[arg(long, env = "BITCOIN_RPC_PASSWORD", default_value = "password")]
    bitcoin_rpc_password: String,
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

/// Perform round 1 signing: generate nonces and commitments for each input
fn do_round1_signing(
    psbt: &mut Psbt,
    key_package: &frost::keys::KeyPackage,
    identifier: frost::Identifier,
) -> anyhow::Result<Vec<(SigningNonces, SigningCommitments)>> {
    // Basic PSBT sanity checks
    if psbt.inputs.is_empty() {
        return Err(anyhow::anyhow!("PSBT must have at least one input"));
    }

    if psbt.outputs.is_empty() {
        return Err(anyhow::anyhow!("PSBT must have at least one output"));
    }

    // Basic fee sanity check
    validate_psbt_fee_sanity(psbt)?;

    // Generate nonces and commitments for each input
    let num_inputs = psbt.inputs.len();
    let secret = key_package.signing_share();
    let mut nonces = vec![];
    let mut rng = thread_rng();

    // Order is important - each nonce pair corresponds to a transaction input
    for i in 0..num_inputs {
        let nonce_pkg = frost::round1::commit(secret, &mut rng);
        psbt.inputs[i].set_signing_commitment(identifier, &nonce_pkg.1);
        nonces.push(nonce_pkg);
    }

    Ok(nonces)
}

/// Perform round 2 signing: generate partial signatures for each input
fn do_round2_signing(
    psbt: &mut Psbt,
    key_package: &frost::keys::KeyPackage,
    identifier: frost::Identifier,
    signing_nonces: &[(SigningNonces, SigningCommitments)],
) -> anyhow::Result<()> {
    let num_inputs = psbt.inputs.len();
    if signing_nonces.len() != num_inputs {
        return Err(anyhow::anyhow!(
            "Number of signing nonces ({}) does not match number of inputs ({})",
            signing_nonces.len(),
            num_inputs
        ));
    }

    // Get signing packages from the PSBT (which now has commitments from all signers)
    let signing_packages = psbt.signing_packages()?;

    // Generate partial signature for each input
    for (index, (signing_package, psbt_in)) in
        signing_packages.iter().zip(psbt.inputs.iter_mut()).enumerate()
    {
        // Check if this signer is in the signing set
        let signing_commitments = signing_package.signing_commitments();
        if !signing_commitments.contains_key(&identifier) {
            return Err(anyhow::anyhow!("Signer not found in signing package at index {}", index));
        }

        // Get the eth_address tweak if present
        let eth_address_tweak = psbt_in.eth_address();

        // Create signing parameters with the tweak
        let signing_parameters = SigningParameters {
            tapscript_merkle_root: None,
            additional_tweak: eth_address_tweak.map(|e| e.to_vec()),
        };

        // Generate partial signature
        let sig = frost::round2::sign_with_tweak(
            signing_package,
            &signing_nonces.get(index).expect("valid index").0,
            key_package,
            &signing_parameters,
        )?;

        // Store the partial signature
        psbt_in.set_partial_signature(identifier, &sig);
    }

    Ok(())
}

/// Aggregate partial signatures and finalize the PSBT into a ready-to-broadcast transaction
fn aggregate_and_finalize(
    psbt: &mut Psbt,
    pk_package: &frost::keys::PublicKeyPackage,
) -> anyhow::Result<bitcoin::Transaction> {
    // Get signing packages for aggregation
    let signing_packages = psbt.signing_packages()?;

    // Aggregate signatures for each input
    for (index, psbt_input) in psbt.inputs.iter_mut().enumerate() {
        let signing_package =
            signing_packages.get(index).ok_or_else(|| {
                anyhow::anyhow!("Missing signing package at index {}", index)
            })?;

        // Collect all partial signatures for this input
        let partial_sigs = psbt_input.all_partial_signatures();

        // Get eth_address tweak if present
        let eth_address_tweak = psbt_input.eth_address();
        let signing_parameters = SigningParameters {
            tapscript_merkle_root: None,
            additional_tweak: eth_address_tweak.map(|e| e.to_vec()),
        };

        // Aggregate the partial signatures
        let agg_sig =
            frost::aggregate_with_tweak(signing_package, &partial_sigs, pk_package, &signing_parameters)?;

        // Verify the aggregated signature
        let effective_key = pk_package.clone().tweak(&signing_parameters);
        effective_key.verifying_key().verify(signing_package.message(), &agg_sig)?;

        // Convert to bitcoin schnorr signature
        let secp_sig = bitcoin::secp256k1::schnorr::Signature::from_slice(&agg_sig.serialize()?)?;

        // Add signature to PSBT
        let hash_ty = bitcoin::sighash::TapSighashType::Default;
        let sighash_type = bitcoin::psbt::PsbtSighashType::from(hash_ty);
        psbt_input.sighash_type = Some(sighash_type);
        psbt_input.tap_key_sig =
            Some(bitcoin::taproot::Signature { signature: secp_sig, sighash_type: hash_ty });
    }

    // Keep a copy of the original psbt as we need to add back the signing commitments and
    // partial signatures - `finalize_mut` removes everything that is not a witness
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let mut original_psbt = psbt.clone();

    // Finalize the PSBT
    if let Err(errs) = MiniscriptPsbtExt::finalize_mut(psbt, &secp) {
        return Err(anyhow::anyhow!("PSBT finalization failed with {} errors: {:?}", errs.len(), errs));
    }

    // Copy finalized witness data back to original PSBT
    for (index, input) in original_psbt.inputs.iter_mut().enumerate() {
        let final_witness = psbt
            .inputs
            .get(index)
            .and_then(|i| i.final_script_witness.clone());
        
        input.final_script_witness = final_witness;
    }

    // Extract the final transaction
    let tx = original_psbt.extract_tx()?;
    Ok(tx)
}

/// Main service implementation
#[derive(Clone)]
pub struct PeginRecoveryServiceImpl {
    db: recovery_db::Db,
    bitcoind_client: Arc<bitcoincore_rpc::Client>,
}

impl PeginRecoveryServiceImpl {
    pub fn new(db: recovery_db::Db, bitcoind_client: Arc<bitcoincore_rpc::Client>) -> Self {
        Self { db, bitcoind_client }
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
            "RecoverPegin requested - destination: {}, txid: {}, vout: {}, multisig_id: {} bytes",
            req.destination,
            req.txid,
            req.vout,
            req.multisig_id.len()
        );

        let testnet = true; // TODO: get from config
        let multisig_id = &req.multisig_id;

        if multisig_id.is_empty() {
            return Err(badarg!("multisig_id is required"));
        }

        // Load key shares and public key package for this multisig
        let multisig_shares = self
            .db
            .get_key_shares(multisig_id)
            .map_err(|e| Status::internal(format!("Failed to get key shares: {}", e)))?
            .ok_or_else(|| badarg!("No key shares found for multisig_id"))?;

        let pk_package = self
            .db
            .get_public_key_package(multisig_id)
            .map_err(|e| Status::internal(format!("Failed to get public key package: {}", e)))?
            .ok_or_else(|| badarg!("No public key package found for multisig_id"))?;

        // Use all available key shares for signing
        // In a proper recovery scenario, we need at least min_signers shares,
        // but since we don't have a direct way to get that from PublicKeyPackage,
        // we'll use all shares we have. The FROST protocol will validate if we have enough.
        let num_shares = multisig_shares.shares.len();
        info!("Using {} key shares for signing", num_shares);

        if num_shares == 0 {
            return Err(badarg!("No key shares available for signing"));
        }

        // Select all available key shares
        let selected_shares: Vec<_> = multisig_shares.shares.iter().collect();
        info!("Selected {} key shares for signing", selected_shares.len());

        // Parse and validate the request
        let destination = parse_and_validate_address(&req.destination, testnet)
            .map_err(|e| badarg!("Invalid destination: {}", e))?;
        let script_pubkey = destination.script_pubkey();

        let txid = req.txid.parse::<bitcoin::Txid>().map_err(|e| badarg!("Invalid txid: {}", e))?;
        let vout = req.vout;
        let eth_address = parse_eth_address(req.eth_address)
            .map_err(|e| badarg!("Invalid eth address: {}", e))?;

        let outpoint = OutPoint::new(txid, vout);

        // Validate UTXO exists on chain
        info!("Validating UTXO {} exists on chain", outpoint);
        let on_chain_utxo = self
            .bitcoind_client
            .get_tx_out(&txid, vout, None)
            .map_err(|e| {
                Status::internal(format!("Failed to query Bitcoin RPC for UTXO {}: {}", outpoint, e))
            })?;

        let on_chain_utxo = on_chain_utxo.ok_or_else(|| {
            badarg!("UTXO {} not found on chain or already spent", outpoint)
        })?;

        info!(
            "UTXO {} found on chain with {} confirmations",
            outpoint, on_chain_utxo.confirmations
        );

        // Extract validated on-chain values
        let input_amount = on_chain_utxo.value;
        let on_chain_script_pubkey =
            ScriptBuf::from_bytes(on_chain_utxo.script_pub_key.hex.clone());

        // Generate expected scriptPubKey from the public key package and eth address
        let agg_key = pk_package.verifying_key();
        let tweaked_key = generate_tweaked_public_key(agg_key, &eth_address).map_err(|e| {
            Status::internal(format!("Failed to generate tweaked public key: {}", e))
        })?;
        let expected_script_pubkey = generate_taproot_scriptpubkey(&tweaked_key);

        // Verify the on-chain scriptPubKey matches what we expect
        if on_chain_script_pubkey != expected_script_pubkey {
            return Err(badarg!(
                "UTXO {} scriptPubKey does not match expected address for eth_address {}",
                outpoint,
                hex::encode(eth_address)
            ));
        }

        info!(
            "UTXO {} validated successfully: amount={}, scriptPubKey matches",
            outpoint, input_amount
        );

        // Create the utxo with validated on-chain data
        let utxo = database::Utxo {
            outpoint,
            output: TxOut { value: input_amount, script_pubkey: expected_script_pubkey.clone() },
            eth_address: Some(eth_address),
            version: 1,
        };
        let utxos = vec![utxo];

        // Calculate the fee and subtract from the output value
        let fee_rate_sat_per_vbyte = 5; // TODO: get from config
        let fee_rate: FeeRate = FeeRate::from_sat_per_vb(fee_rate_sat_per_vbyte)
            .ok_or(badarg!("Invalid fee rate: {}", fee_rate_sat_per_vbyte))?;
        let absolute_fee = calculate_fee(&utxos, &script_pubkey, fee_rate)
            .map_err(|e| badarg!("Invalid fee: {}", e))?;
        let output_value =
            input_amount.checked_sub(absolute_fee).ok_or(badarg!("output value underflow"))?;

        // Create the PSBT
        let mut psbt = create_recovery_psbt(utxos, &script_pubkey, output_value);

        // Round 1: Generate nonces and commitments from each selected key share
        let mut all_nonces = Vec::new();
        for (node_id_bytes, key_package) in selected_shares.iter() {
            let identifier = frost::Identifier::deserialize(
                node_id_bytes.as_slice().try_into().map_err(|_| {
                    Status::internal("Invalid node identifier in key share")
                })?,
            )
            .map_err(|_| Status::internal("Failed to deserialize node identifier"))?;

            let nonces = do_round1_signing(&mut psbt, key_package, identifier)
                .map_err(|e| Status::internal(format!("Round 1 signing failed: {}", e)))?;

            all_nonces.push((identifier, nonces));
            info!("Round 1 complete for identifier: {:?}", identifier);
        }

        // Round 2: Generate partial signatures from each selected key share
        for (identifier, nonces) in all_nonces.iter() {
            let key_package = multisig_shares
                .shares
                .get(&identifier.serialize().to_vec())
                .ok_or_else(|| Status::internal("Key package not found for identifier"))?;

            do_round2_signing(&mut psbt, key_package, *identifier, nonces)
                .map_err(|e| Status::internal(format!("Round 2 signing failed: {}", e)))?;

            info!("Round 2 complete for identifier: {:?}", identifier);
        }

        // Aggregate and finalize
        let final_tx = aggregate_and_finalize(&mut psbt, &pk_package)
            .map_err(|e| Status::internal(format!("Aggregation/finalization failed: {}", e)))?;

        info!("Transaction successfully signed. Txid: {}", final_tx.compute_txid());

        // Serialize the transaction
        let tx_bytes = bitcoin::consensus::serialize(&final_tx);
        let tx_hex = hex::encode(&tx_bytes);

        // TODO: Broadcast the transaction to Bitcoin network

        Ok(Response::new(RecoverPeginResponse {
            tx: tx_hex,
            txid: final_tx.compute_txid().to_string(),
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

    // Create Bitcoin RPC client
    info!("Connecting to Bitcoin RPC at: {}", args.bitcoin_rpc_url);
    let bitcoind_client = bitcoincore_rpc::Client::new(
        &args.bitcoin_rpc_url,
        bitcoincore_rpc::Auth::UserPass(args.bitcoin_rpc_user.clone(), args.bitcoin_rpc_password.clone()),
    )
    .map_err(|e| anyhow::anyhow!("Failed to connect to Bitcoin RPC: {}", e))?;
    
    // Verify connection by getting blockchain info
    let blockchain_info = bitcoind_client.get_blockchain_info()
        .map_err(|e| anyhow::anyhow!("Failed to get blockchain info from Bitcoin RPC: {}", e))?;
    info!(
        "Connected to Bitcoin RPC - Chain: {}, Blocks: {}", 
        blockchain_info.chain, 
        blockchain_info.blocks
    );

    // Configure service address
    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    info!("gRPC server listening on {}", addr);

    // Create service
    let service = PeginRecoveryServiceImpl::new(db, Arc::new(bitcoind_client));
    let svc = PeginRecoveryServiceServer::new(service);

    // Configure reflection (for grpcurl and similar tools)
    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(FILE_DESCRIPTOR_SET)
        .build_v1()?;

    // Start server
    Server::builder().add_service(svc).add_service(reflection_service).serve(addr).await?;

    Ok(())
}
