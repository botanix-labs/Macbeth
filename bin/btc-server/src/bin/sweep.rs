use std::{fs::File, io::Write, path::PathBuf};

use base64::prelude::*;
use bitcoin::{consensus::encode::serialize_hex, psbt::Psbt, Amount, FeeRate, OutPoint, TxOut};
use btcserverlib::{
    database,
    database::version::UtxoVersion,
    wallet::{
        psbt::{PsbtExt, PsbtInputExt},
        util::calculate_signed_tx_weight,
    },
};
use clap::Parser;
use frost_secp256k1_tr as frost;
use frost_secp256k1_tr::{
    keys::Tweak,
    round1::{SigningCommitments, SigningNonces},
    SigningParameters,
};
use rand::{thread_rng, RngCore};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

// Constants
const DEFAULT_NONCE_FILE_PERMISSIONS: u32 = 0o600;
const FROST_ID_PREFIX_LENGTH: usize = 6;
const SESSION_ID_PREFIX_LENGTH: usize = 12;
const DUMMY_IDENTIFIER_SIZE: usize = 33;
const SIGNING_SESSION_ID_SIZE: usize = 32;

#[derive(Clone, Debug, Parser)]
#[command(name = "sweep")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Commands,
}

#[derive(Clone, Debug, Parser)]
pub enum Commands {
    #[command(name = "coordinator-1-create-psbt")]
    Coordinator1CreatePsbt(CreatePsbtConfig),
    #[command(name = "signer-1-generate-commitments")]
    Signer1GenerateCommitments(GenerateCommitmentsConfig),
    #[command(name = "coordinator-2-collect-commitments")]
    Coordinator2CollectCommitments(CollectCommitmentsConfig),
    #[command(name = "signer-2-generate-signatures")]
    Signer2GenerateSignatures(GenerateSignaturesConfig),
    #[command(name = "coordinator-3-finalize-transaction")]
    Coordinator3FinalizeTransaction(FinalizeTransactionConfig),
    #[command(name = "utils-add-dummy-utxos")]
    UtilsAddDummyUtxos(AddDummyUtxosConfig),
}

#[derive(Clone, Debug, Parser)]
pub struct CreatePsbtConfig {
    #[arg(long)]
    pub db: PathBuf,
    #[arg(long)]
    pub output_address: String,
    #[arg(long)]
    pub sat_per_vbyte: u64,
    /// Expects address to be testnet
    #[arg(long)]
    pub testnet: bool,
}

#[derive(Clone, Debug, Parser)]
pub struct GenerateCommitmentsConfig {
    #[arg(long)]
    pub input_json: PathBuf,
    #[arg(long)]
    pub db: PathBuf,
    #[arg(long)]
    pub identifier: u16, // TODO: get this from the same config as btc-server
}

#[derive(Clone, Debug, Parser)]
pub struct CollectCommitmentsConfig {
    /// List of Round 1 response JSON files from signers
    #[arg(long, value_delimiter = ',')]
    pub round1_responses: Vec<PathBuf>,
    /// Minimum number of signers required for threshold
    #[arg(long)]
    pub min_signers: u16,
    /// Output JSON file for the combined signing package
    #[arg(long, default_value = "signing_package_round2.json")]
    pub output_json: PathBuf,
    /// Database path for validation
    #[arg(long)]
    pub db: PathBuf,
}

#[derive(Clone, Debug, Parser)]
pub struct GenerateSignaturesConfig {
    /// Input JSON file from coordinator (signing_package_round2.json)
    #[arg(long)]
    pub input_json: PathBuf,
    /// Nonces JSON file saved from Round 1 (nonces_*.json)
    #[arg(long)]
    pub nonces_json: PathBuf,
    /// Database path for key package
    #[arg(long)]
    pub db: PathBuf,
    /// FROST identifier (same as used in Round 1)
    #[arg(long)]
    pub identifier: u16,
}

#[derive(Clone, Debug, Parser)]
pub struct FinalizeTransactionConfig {
    /// List of Round 2 response JSON files from signers  
    #[arg(long, value_delimiter = ',')]
    pub round2_responses: Vec<PathBuf>,
    /// Minimum number of signers required for threshold
    #[arg(long)]
    pub min_signers: u16,
    /// Output file for the finalized transaction hex
    #[arg(long, default_value = "finalized_transaction.hex")]
    pub output_file: PathBuf,
    /// Database path for validation
    #[arg(long)]
    pub db: PathBuf,
}

#[derive(Clone, Debug, Parser)]
pub struct AddDummyUtxosConfig {
    #[arg(long)]
    pub db: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct SigningPackage {
    psbt_base64: String,
    identifier_hex: String,
    signing_session_id_hex: String,
}

#[derive(Serialize, Deserialize)]
struct StoredNonces {
    signing_session_id_hex: String,
    frost_identifier_hex: String,
    nonces: Vec<NonceEntry>,
}

#[derive(Serialize, Deserialize)]
struct NonceEntry {
    input_index: usize,
    signing_nonces_hex: String, // TODO: encrypt this sensitive data
    signing_commitments_hex: String,
}

fn validate_psbt_fee_sanity(psbt: &Psbt) -> anyhow::Result<()> {
    let fee = psbt.fee().map_err(|e| anyhow::anyhow!("Failed to calculate PSBT fee: {}", e))?;

    let total_outputs_amount =
        psbt.unsigned_tx.output.iter().fold(Amount::ZERO, |total, output| {
            total.checked_add(output.value).unwrap_or_default()
        });

    if fee > total_outputs_amount {
        return Err(anyhow::anyhow!(
            "Fee ({}) cannot be greater than total output value ({})",
            fee,
            total_outputs_amount
        ));
    }

    Ok(())
}

/// Save secret nonces to JSON file for Round 2 usage
/// TODO: Encrypt the sensitive nonce data
fn save_nonces_to_file(
    nonces: &[(SigningNonces, SigningCommitments)],
    signing_session_id: &[u8; 32],
    frost_identifier: frost::Identifier,
) -> anyhow::Result<String> {
    let frost_id_prefix =
        hex::encode(frost_identifier.serialize())[..FROST_ID_PREFIX_LENGTH].to_string();
    let session_id_prefix = hex::encode(signing_session_id)[..SESSION_ID_PREFIX_LENGTH].to_string();
    let filename = format!("nonces_{}_{}.json", frost_id_prefix, session_id_prefix);

    let nonce_entries: Vec<NonceEntry> = nonces
        .iter()
        .enumerate()
        .map(|(index, (signing_nonces, signing_commitments))| NonceEntry {
            input_index: index,
            signing_nonces_hex: hex::encode(
                signing_nonces.serialize().expect("nonce serialization"),
            ),
            signing_commitments_hex: hex::encode(
                signing_commitments.serialize().expect("commitment serialization"),
            ),
        })
        .collect();

    let stored_nonces = StoredNonces {
        signing_session_id_hex: hex::encode(signing_session_id),
        frost_identifier_hex: hex::encode(frost_identifier.serialize()),
        nonces: nonce_entries,
    };

    let json = serde_json::to_string_pretty(&stored_nonces)
        .map_err(|e| anyhow::anyhow!("Failed to serialize nonces: {}", e))?;

    std::fs::write(&filename, json)
        .map_err(|e| anyhow::anyhow!("Failed to write nonces file: {}", e))?;

    // Set restrictive permissions (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&filename)?.permissions();
        perms.set_mode(DEFAULT_NONCE_FILE_PERMISSIONS); // rw-------
        std::fs::set_permissions(&filename, perms)?;
    }

    Ok(filename)
}

/// Load secret nonces from JSON file for Round 2 usage
fn load_nonces_from_file(filename: &str) -> anyhow::Result<StoredNonces> {
    let content = std::fs::read_to_string(filename)
        .map_err(|e| anyhow::anyhow!("Failed to read nonces file {}: {}", filename, e))?;

    serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Failed to parse nonces file: {}", e))
}

/// Sweep-specific version of FROST round 1 signing that bypasses btc-server validations
/// This is designed for sweep transactions which don't follow normal btc-server patterns
fn get_round1_signing_package_sweep(
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

fn parse_and_validate_address(
    address_str: &str,
    testnet: bool,
) -> anyhow::Result<bitcoin::Address> {
    let network = if testnet { bitcoin::Network::Testnet } else { bitcoin::Network::Bitcoin };

    bitcoin::Address::from_str(address_str)
        .map_err(|e| anyhow::anyhow!("invalid address: {}", e))?
        .require_network(network)
        .map_err(|e| anyhow::anyhow!("address network error: {}", e))
}

// Based on wallet::psbt::create_psbt but with a single output with no pegout id
pub(crate) fn create_sweep_psbt(
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

fn calculate_sweep_fee(
    utxos: &[database::Utxo],
    script_pubkey: &bitcoin::ScriptBuf,
    fee_rate: FeeRate,
) -> anyhow::Result<Amount> {
    let psbt = create_sweep_psbt(utxos.to_vec(), &script_pubkey, Amount::from_sat(0));
    let total_weight = calculate_signed_tx_weight(&psbt)?;
    let absolute_fee = fee_rate.fee_wu(total_weight).ok_or(anyhow::anyhow!("fee rate overflow"))?;
    Ok(absolute_fee)
}

#[tokio::main]
async fn main() -> anyhow::Result<(), anyhow::Error> {
    let cli = Cli::parse();
    match cli.cmd {
        Commands::Coordinator1CreatePsbt(config) => handle_make_sweep_psbt(&config).await?,
        Commands::Signer1GenerateCommitments(config) => handle_frost_round_1(&config).await?,
        Commands::Coordinator2CollectCommitments(config) => {
            handle_frost_coordinator_round_1(&config).await?
        }
        Commands::Signer2GenerateSignatures(config) => handle_frost_round_2(&config).await?,
        Commands::Coordinator3FinalizeTransaction(config) => {
            handle_finalize_transaction(&config).await?
        }
        Commands::UtilsAddDummyUtxos(config) => handle_add_dummy_utxos(&config).await?,
    }

    Ok(())
}

pub async fn handle_make_sweep_psbt(c: &CreatePsbtConfig) -> anyhow::Result<(), anyhow::Error> {
    // get all utxos from the database
    let db = database::Db::open(&c.db).expect("failed to open db");
    let utxos: Vec<database::Utxo> = db.iter_utxos().collect::<Result<Vec<_>, _>>()?;

    if utxos.is_empty() {
        println!("no utxos found");
        return Err(anyhow::anyhow!("no utxos found"));
    }
    println!("utxos = {:?}", utxos);

    // Long term plans, not for now
    // - sort utxos by value
    // - truncate utxos to largest 1000 utxos

    let address = parse_and_validate_address(&c.output_address, c.testnet)?;
    let script_pubkey = address.script_pubkey();

    // calculate the fee and subtract from the output value
    let fee_rate: FeeRate = FeeRate::from_sat_per_vb(c.sat_per_vbyte).expect("fee rate overflow");
    let absolute_fee = calculate_sweep_fee(&utxos, &script_pubkey, fee_rate)?;
    println!("absolute fee = {:?}", absolute_fee);
    let total_utxo_value = utxos.iter().map(|u| u.output.value).sum::<Amount>();
    let output_value = total_utxo_value
        .checked_sub(absolute_fee)
        .ok_or(anyhow::anyhow!("output value underflow"))?;

    let psbt = create_sweep_psbt(utxos, &script_pubkey, output_value);

    let psbt_json = serde_json::to_string(&psbt).expect("failed to serialize psbt");
    let mut file = File::create("psbt.json").expect("failed to create file");
    file.write_all(psbt_json.as_bytes()).expect("failed to write to file");

    println!("psbt tx hex = {}", serialize_hex(&psbt.unsigned_tx));

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

    // serialize to JSON and save to file
    let signing_package_json = serde_json::to_string_pretty(&signing_package)
        .map_err(|e| anyhow::anyhow!("Failed to serialize SigningPackage to JSON: {}", e))?;

    let mut signing_package_file = File::create("signing_package.json")
        .map_err(|e| anyhow::anyhow!("Failed to create signing_package.json: {}", e))?;

    signing_package_file
        .write_all(signing_package_json.as_bytes())
        .map_err(|e| anyhow::anyhow!("Failed to write to signing_package.json: {}", e))?;

    println!("SigningPackage saved to signing_package.json");
    println!("Signing session ID: {}", hex::encode(signing_session_id));

    Ok(())
}

pub async fn handle_frost_round_1(
    c: &GenerateCommitmentsConfig,
) -> anyhow::Result<(), anyhow::Error> {
    // Read the input JSON file
    let input_json = std::fs::read_to_string(&c.input_json)
        .map_err(|e| anyhow::anyhow!("Failed to read input JSON file: {}", e))?;

    let input_package: SigningPackage = serde_json::from_str(&input_json)
        .map_err(|e| anyhow::anyhow!("Failed to parse input JSON: {}", e))?;

    // Parse the signing session ID
    let signing_session_id = hex::decode(&input_package.signing_session_id_hex)
        .map_err(|e| anyhow::anyhow!("Failed to decode signing session ID: {}", e))?;

    if signing_session_id.len() != SIGNING_SESSION_ID_SIZE {
        return Err(anyhow::anyhow!("Invalid signing session ID length"));
    }

    let signing_session_id: [u8; SIGNING_SESSION_ID_SIZE] = signing_session_id.try_into().unwrap();

    // Deserialize the PSBT
    let mut psbt = Psbt::deserialize(&BASE64_STANDARD.decode(&input_package.psbt_base64)?)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize PSBT: {}", e))?;

    // Open database and get key package
    let db =
        database::Db::open(&c.db).map_err(|e| anyhow::anyhow!("Failed to open database: {}", e))?;

    let key_package = db
        .get_key_package()
        .map_err(|e| anyhow::anyhow!("Failed to get key package from database: {}", e))?
        .ok_or(anyhow::anyhow!("No key package found in database"))?;

    // Derive FROST identifier from config (same as main code)
    let frost_identifier = frost::Identifier::derive(c.identifier.to_le_bytes().as_slice())
        .map_err(|e| anyhow::anyhow!("Failed to derive FROST identifier: {}", e))?;

    println!("Processing FROST round 1 for identifier: {:?}", frost_identifier);
    println!("Signing session ID: {}", hex::encode(signing_session_id));

    // Use our sweep-specific FROST round 1 function (bypasses btc-server validations)
    let nonces = get_round1_signing_package_sweep(&mut psbt, &key_package, frost_identifier)
        .map_err(|e| anyhow::anyhow!("Failed to process round 1 signing: {}", e))?;

    println!("Generated {} nonce pairs for {} inputs", nonces.len(), psbt.inputs.len());

    // Save secret nonces for Round 2
    let nonces_filename = save_nonces_to_file(&nonces, &signing_session_id, frost_identifier)
        .map_err(|e| anyhow::anyhow!("Failed to save nonces: {}", e))?;

    println!("Secret nonces saved to: {}", nonces_filename);

    // Create response
    let response = SigningPackage {
        psbt_base64: BASE64_STANDARD.encode(psbt.serialize()),
        identifier_hex: hex::encode(frost_identifier.serialize()),
        signing_session_id_hex: hex::encode(signing_session_id),
    };

    // Save response to JSON file with FROST ID prefix
    let frost_id_prefix =
        hex::encode(frost_identifier.serialize())[..FROST_ID_PREFIX_LENGTH].to_string();
    let output_filename = format!("round_1_response_{}.json", frost_id_prefix);

    let response_json = serde_json::to_string_pretty(&response)
        .map_err(|e| anyhow::anyhow!("Failed to serialize response: {}", e))?;

    std::fs::write(&output_filename, response_json)
        .map_err(|e| anyhow::anyhow!("Failed to write response JSON: {}", e))?;

    println!("FROST round 1 response saved to: {}", output_filename);

    // for debugging, save psbt to a json file with FROST ID prefix
    let psbt_json = serde_json::to_string_pretty(&psbt)
        .map_err(|e| anyhow::anyhow!("Failed to serialize PSBT: {}", e))?;
    let psbt_filename = format!("psbt_after_round_1_{}.json", frost_id_prefix);
    std::fs::write(&psbt_filename, psbt_json)
        .map_err(|e| anyhow::anyhow!("Failed to write PSBT JSON: {}", e))?;
    println!("psbt after round 1 saved to: {}", psbt_filename);

    Ok(())
}

pub async fn handle_frost_coordinator_round_1(
    c: &CollectCommitmentsConfig,
) -> anyhow::Result<(), anyhow::Error> {
    println!(
        "Collecting {} Round 1 responses with min_signers={}",
        c.round1_responses.len(),
        c.min_signers
    );

    // Validate we have enough responses
    if c.round1_responses.len() < c.min_signers as usize {
        return Err(anyhow::anyhow!(
            "Not enough Round 1 responses: got {}, need at least {}",
            c.round1_responses.len(),
            c.min_signers
        ));
    }

    // Load and parse all Round 1 response files
    let mut signer_responses = Vec::new();
    for response_file in &c.round1_responses {
        println!("Loading Round 1 response from: {}", response_file.display());

        let content = std::fs::read_to_string(response_file)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", response_file.display(), e))?;

        let response: SigningPackage = serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", response_file.display(), e))?;

        signer_responses.push(response);
    }

    // All responses should have the same signing session ID and base PSBT structure
    let reference_session_id = &signer_responses[0].signing_session_id_hex;
    let reference_psbt_base64 = &signer_responses[0].psbt_base64;

    for (i, response) in signer_responses.iter().enumerate().skip(1) {
        if response.signing_session_id_hex != *reference_session_id {
            return Err(anyhow::anyhow!(
                "Signing session ID mismatch in file {}: expected {}, got {}",
                c.round1_responses[i].display(),
                reference_session_id,
                response.signing_session_id_hex
            ));
        }
    }

    // Start with the first PSBT and merge commitments from others
    let mut combined_psbt = Psbt::deserialize(&BASE64_STANDARD.decode(reference_psbt_base64)?)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize reference PSBT: {}", e))?;

    println!("Base PSBT has {} inputs", combined_psbt.inputs.len());

    // For each additional response, merge their commitments into our combined PSBT
    for (i, response) in signer_responses.iter().enumerate().skip(1) {
        let signer_psbt = Psbt::deserialize(&BASE64_STANDARD.decode(&response.psbt_base64)?)
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to deserialize PSBT from {}: {}",
                    c.round1_responses[i].display(),
                    e
                )
            })?;

        // Verify structure matches
        if signer_psbt.inputs.len() != combined_psbt.inputs.len() {
            return Err(anyhow::anyhow!(
                "Input count mismatch in {}: expected {}, got {}",
                c.round1_responses[i].display(),
                combined_psbt.inputs.len(),
                signer_psbt.inputs.len()
            ));
        }

        // Merge proprietary fields (commitments) from signer's PSBT into combined PSBT
        for (input_idx, signer_input) in signer_psbt.inputs.iter().enumerate() {
            let combined_input = &mut combined_psbt.inputs[input_idx];

            // Copy over all proprietary fields from this signer
            for (key, value) in &signer_input.proprietary {
                if combined_input.proprietary.contains_key(key) {
                    // Only error on duplicate FROST signing commitment keys (subtype 2)
                    // Other fields like ETH addresses (subtype 1) or UTXO versions (subtype 4) are
                    // expected to be the same
                    if key.prefix == b"btx" && key.subtype == 2 {
                        let frost_id_hex = hex::encode(&key.key);
                        return Err(anyhow::anyhow!(
                            "Duplicate FROST identifier found in input {}: FROST_ID:{}. \
                             This means two signers have the same identifier. \
                             Each signer must have a unique --identifier value. \
                             Check your Round 1 response files for duplicate identifiers.",
                            input_idx,
                            frost_id_hex
                        ));
                    }
                }
                combined_input.proprietary.insert(key.clone(), value.clone());
            }
        }

        println!("Merged commitments from {}", c.round1_responses[i].display());
    }

    // Validate we have enough commitments for each input
    for (input_idx, input) in combined_psbt.inputs.iter().enumerate() {
        let commitment_count = input
            .proprietary
            .iter()
            .filter(|(key, _)| {
                // Count signing commitment proprietary fields (subtype 2)
                key.prefix == b"btx" && key.subtype == 2
            })
            .count();

        println!("Input {}: {} signing commitments", input_idx, commitment_count);

        if commitment_count < c.min_signers as usize {
            return Err(anyhow::anyhow!(
                "Input {} has insufficient commitments: got {}, need {}",
                input_idx,
                commitment_count,
                c.min_signers
            ));
        }
    }

    // Create the final signing package for Round 2
    let signing_session_id = hex::decode(reference_session_id)
        .map_err(|e| anyhow::anyhow!("Failed to decode signing session ID: {}", e))?;

    if signing_session_id.len() != SIGNING_SESSION_ID_SIZE {
        return Err(anyhow::anyhow!("Invalid signing session ID length"));
    }

    let round2_package = SigningPackage {
        psbt_base64: BASE64_STANDARD.encode(combined_psbt.serialize()),
        identifier_hex: "combined".to_string(), // Coordinator identifier
        signing_session_id_hex: reference_session_id.clone(),
    };

    // Save the combined signing package
    let package_json = serde_json::to_string_pretty(&round2_package)
        .map_err(|e| anyhow::anyhow!("Failed to serialize signing package: {}", e))?;

    std::fs::write(&c.output_json, package_json)
        .map_err(|e| anyhow::anyhow!("Failed to write output file: {}", e))?;

    println!("✅ Combined signing package saved to: {}", c.output_json.display());
    println!("   - Inputs: {}", combined_psbt.inputs.len());
    println!("   - Total signers: {}", signer_responses.len());
    println!("   - Min signers: {}", c.min_signers);

    // Save debug PSBT
    let debug_psbt_filename = c.output_json.with_extension("psbt.json");
    let psbt_json = serde_json::to_string_pretty(&combined_psbt)
        .map_err(|e| anyhow::anyhow!("Failed to serialize debug PSBT: {}", e))?;

    std::fs::write(&debug_psbt_filename, psbt_json)
        .map_err(|e| anyhow::anyhow!("Failed to write debug PSBT: {}", e))?;

    println!("📋 Debug PSBT saved to: {}", debug_psbt_filename.display());

    Ok(())
}

pub async fn handle_frost_round_2(
    c: &GenerateSignaturesConfig,
) -> anyhow::Result<(), anyhow::Error> {
    println!("Processing FROST Round 2 signing");
    println!("  Input JSON: {}", c.input_json.display());
    println!("  Nonces JSON: {}", c.nonces_json.display());
    println!("  Database: {}", c.db.display());
    println!("  Identifier: {}", c.identifier);

    // 1. Load input JSON (signing_package_round2.json)
    println!("Loading coordinator's signing package from: {}", c.input_json.display());

    let input_content = std::fs::read_to_string(&c.input_json)
        .map_err(|e| anyhow::anyhow!("Failed to read input JSON file: {}", e))?;

    let input_package: SigningPackage = serde_json::from_str(&input_content)
        .map_err(|e| anyhow::anyhow!("Failed to parse input JSON: {}", e))?;

    println!("Successfully loaded signing package:");
    println!("  Session ID: {}", input_package.signing_session_id_hex);
    println!("  Identifier: {}", input_package.identifier_hex);

    // Deserialize the PSBT with all commitments
    let psbt = Psbt::deserialize(&BASE64_STANDARD.decode(&input_package.psbt_base64)?)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize PSBT: {}", e))?;

    println!("PSBT loaded with {} inputs", psbt.inputs.len());

    // 2. Load nonces file
    println!("Loading secret nonces from: {}", c.nonces_json.display());

    let stored_nonces = load_nonces_from_file(&c.nonces_json.to_string_lossy())
        .map_err(|e| anyhow::anyhow!("Failed to load nonces file: {}", e))?;

    println!("Successfully loaded nonces:");
    println!("  Session ID: {}", stored_nonces.signing_session_id_hex);
    println!("  FROST ID: {}", stored_nonces.frost_identifier_hex);
    println!("  Nonce pairs: {}", stored_nonces.nonces.len());

    // Validate session ID matches
    if stored_nonces.signing_session_id_hex != input_package.signing_session_id_hex {
        return Err(anyhow::anyhow!(
            "Session ID mismatch: nonces file has {}, input has {}",
            stored_nonces.signing_session_id_hex,
            input_package.signing_session_id_hex
        ));
    }

    // Validate number of nonces matches number of inputs
    if stored_nonces.nonces.len() != psbt.inputs.len() {
        return Err(anyhow::anyhow!(
            "Nonce count mismatch: {} nonces for {} inputs",
            stored_nonces.nonces.len(),
            psbt.inputs.len()
        ));
    }

    println!("Nonces validation passed");

    // 3. Load key package from database
    println!("Loading key package from database: {}", c.db.display());

    let db =
        database::Db::open(&c.db).map_err(|e| anyhow::anyhow!("Failed to open database: {}", e))?;

    let key_package = db
        .get_key_package()
        .map_err(|e| anyhow::anyhow!("Failed to get key package from database: {}", e))?
        .ok_or(anyhow::anyhow!("No key package found in database"))?;

    println!("Successfully loaded key package from database");
    println!("Key package identifier: {}", hex::encode(key_package.identifier().serialize()));

    // Derive FROST identifier from config (same as Round 1)
    let frost_identifier = frost::Identifier::derive(c.identifier.to_le_bytes().as_slice())
        .map_err(|e| anyhow::anyhow!("Failed to derive FROST identifier: {}", e))?;

    println!("Derived FROST identifier: {}", hex::encode(frost_identifier.serialize()));

    // Validate our FROST identifier matches the nonces file
    let expected_frost_id = hex::encode(frost_identifier.serialize());
    if stored_nonces.frost_identifier_hex != expected_frost_id {
        return Err(anyhow::anyhow!(
            "FROST identifier mismatch: nonces file has {}, derived {}",
            stored_nonces.frost_identifier_hex,
            expected_frost_id
        ));
    }

    println!("FROST identifier validation passed");

    let mut psbt_copy = psbt.clone();

    // Convert stored nonces back to FROST types
    let mut signing_nonces_vec = Vec::new();
    for nonce_data in &stored_nonces.nonces {
        let signing_nonces =
            SigningNonces::deserialize(&hex::decode(&nonce_data.signing_nonces_hex)?)
                .map_err(|e| anyhow::anyhow!("Failed to deserialize signing nonces: {}", e))?;
        let signing_commitments =
            SigningCommitments::deserialize(&hex::decode(&nonce_data.signing_commitments_hex)?)
                .map_err(|e| anyhow::anyhow!("Failed to deserialize signing commitments: {}", e))?;

        signing_nonces_vec.push((signing_nonces, signing_commitments));
    }

    println!("Successfully loaded {} nonce pairs", signing_nonces_vec.len());

    // Get signing packages from PSBT (same as btc-server)
    let mut signing_packages = psbt_copy
        .signing_packages()
        .map_err(|e| anyhow::anyhow!("Failed to get signing packages from PSBT: {}", e))?;

    println!("Generated {} signing packages from PSBT", signing_packages.len());

    // Validate nonce count matches input count
    if signing_nonces_vec.len() != psbt_copy.inputs.len() {
        return Err(anyhow::anyhow!(
            "Number of signing nonces ({}) does not match number of inputs ({})",
            signing_nonces_vec.len(),
            psbt_copy.inputs.len()
        ));
    }

    // Validate that our signer is in each signing package and nonces match
    for (index, signing_package) in signing_packages.iter().enumerate() {
        let signing_commitments = signing_package.signing_commitments();
        if !signing_commitments.contains_key(&frost_identifier) {
            return Err(anyhow::anyhow!("Signer not found in signing package for input {}", index));
        }

        let our_sc = signing_commitments
            .get(&frost_identifier)
            .ok_or(anyhow::anyhow!("Failed to get our signing commitment for input {}", index))?;
        let our_nonce = signing_nonces_vec
            .get(index)
            .ok_or(anyhow::anyhow!("Failed to get our nonce for input {}", index))?;

        if our_sc != &our_nonce.1 {
            return Err(anyhow::anyhow!("Invalid nonce pair for input {}", index));
        }
    }

    // Generate partial signature for each input (following btc-server logic)
    for (index, (signing_package, psbt_in)) in
        signing_packages.iter_mut().zip(psbt_copy.inputs.iter_mut()).enumerate()
    {
        let eth_address_tweak = psbt_in.eth_address();

        // Create signing parameters with eth_address tweak if present
        let signing_parameters = SigningParameters {
            tapscript_merkle_root: None,
            additional_tweak: eth_address_tweak.map(|e| e.to_vec()),
        };

        println!("Generating partial signature for input {}", index);

        // Generate the partial signature using FROST
        let sig = frost::round2::sign_with_tweak(
            signing_package,
            &signing_nonces_vec.get(index).expect("valid index").0,
            &key_package,
            &signing_parameters,
        )
        .map_err(|e| {
            anyhow::anyhow!("Failed to generate partial signature for input {}: {}", index, e)
        })?;

        // Set the partial signature in the PSBT
        psbt_in.set_partial_signature(frost_identifier, &sig);
    }

    // 5. Save Round 2 response with identifier prefix

    let frost_id_hex = hex::encode(frost_identifier.serialize());
    let frost_id_short = &frost_id_hex[..FROST_ID_PREFIX_LENGTH];

    let round2_response = SigningPackage {
        psbt_base64: BASE64_STANDARD.encode(psbt_copy.serialize()),
        identifier_hex: frost_id_hex.clone(),
        signing_session_id_hex: stored_nonces.signing_session_id_hex.clone(),
    };

    // Save with identifier prefix like Round 1
    let output_filename = format!("round_2_response_{}.json", frost_id_short);
    let output_path = std::env::current_dir()?.join(output_filename);

    let response_json = serde_json::to_string_pretty(&round2_response)
        .map_err(|e| anyhow::anyhow!("Failed to serialize Round 2 response: {}", e))?;

    std::fs::write(&output_path, response_json)
        .map_err(|e| anyhow::anyhow!("Failed to write Round 2 response file: {}", e))?;

    println!("✅ Round 2 response saved to: {}", output_path.display());
    println!("   - FROST ID: {}", frost_id_hex);
    println!("   - Session ID: {}", stored_nonces.signing_session_id_hex);
    println!("   - Partial signatures: {}", psbt_copy.inputs.len());

    // // Cleanup: remove nonces file for security
    // if std::fs::remove_file(&c.nonces_json).is_ok() {
    //     println!("🗑️  Nonces file removed for security");
    // } else {
    //     println!("⚠️  Warning: Could not remove nonces file - please delete manually");
    // }

    println!("🎉 FROST Round 2 signing complete!");
    Ok(())
}

pub async fn handle_finalize_transaction(
    c: &FinalizeTransactionConfig,
) -> anyhow::Result<(), anyhow::Error> {
    println!("Processing FROST Round 3 - Finalizing transaction");
    println!("  Round 2 responses: {}", c.round2_responses.len());
    println!("  Min signers: {}", c.min_signers);
    println!("  Database: {}", c.db.display());

    // Validate we have enough responses
    if c.round2_responses.len() < c.min_signers as usize {
        return Err(anyhow::anyhow!(
            "Not enough Round 2 responses: got {}, need at least {}",
            c.round2_responses.len(),
            c.min_signers
        ));
    }

    // Load and parse all Round 2 response files
    let mut signer_responses = Vec::new();
    for response_file in &c.round2_responses {
        println!("Loading Round 2 response from: {}", response_file.display());

        let content = std::fs::read_to_string(response_file)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", response_file.display(), e))?;

        let response: SigningPackage = serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", response_file.display(), e))?;

        signer_responses.push(response);
    }

    // All responses should have the same signing session ID
    let reference_session_id = &signer_responses[0].signing_session_id_hex;
    for (i, response) in signer_responses.iter().enumerate().skip(1) {
        if response.signing_session_id_hex != *reference_session_id {
            return Err(anyhow::anyhow!(
                "Signing session ID mismatch in file {}: expected {}, got {}",
                c.round2_responses[i].display(),
                reference_session_id,
                response.signing_session_id_hex
            ));
        }
    }

    // Start with the first PSBT and merge partial signatures from others
    let mut combined_psbt =
        Psbt::deserialize(&BASE64_STANDARD.decode(&signer_responses[0].psbt_base64)?)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize reference PSBT: {}", e))?;

    println!("Base PSBT has {} inputs", combined_psbt.inputs.len());

    // For each additional response, merge their partial signatures into our combined PSBT
    for (i, response) in signer_responses.iter().enumerate().skip(1) {
        let signer_psbt = Psbt::deserialize(&BASE64_STANDARD.decode(&response.psbt_base64)?)
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to deserialize PSBT from {}: {}",
                    c.round2_responses[i].display(),
                    e
                )
            })?;

        // Verify structure matches
        if signer_psbt.inputs.len() != combined_psbt.inputs.len() {
            return Err(anyhow::anyhow!(
                "Input count mismatch in {}: expected {}, got {}",
                c.round2_responses[i].display(),
                combined_psbt.inputs.len(),
                signer_psbt.inputs.len()
            ));
        }

        // Merge partial signatures from signer's PSBT into combined PSBT
        for (input_idx, signer_input) in signer_psbt.inputs.iter().enumerate() {
            let combined_input = &mut combined_psbt.inputs[input_idx];

            // Copy over partial signatures (subtype 3)
            for (key, value) in &signer_input.proprietary {
                if key.prefix == b"btx" && key.subtype == 3 {
                    // Check for duplicate partial signatures
                    if combined_input.proprietary.contains_key(key) {
                        let frost_id_hex = hex::encode(&key.key);
                        return Err(anyhow::anyhow!(
                            "Duplicate partial signature found in input {}: FROST_ID:{}. \
                             This means the same signer provided multiple signatures.",
                            input_idx,
                            frost_id_hex
                        ));
                    }
                    combined_input.proprietary.insert(key.clone(), value.clone());
                }
            }
        }

        println!("Merged partial signatures from {}", c.round2_responses[i].display());
    }

    // Validate we have enough partial signatures for each input
    for (input_idx, input) in combined_psbt.inputs.iter().enumerate() {
        let signature_count = input
            .proprietary
            .iter()
            .filter(|(key, _)| {
                // Count partial signature proprietary fields (subtype 3)
                key.prefix == b"btx" && key.subtype == 3
            })
            .count();

        println!("Input {}: {} partial signatures", input_idx, signature_count);

        if signature_count < c.min_signers as usize {
            return Err(anyhow::anyhow!(
                "Input {} has insufficient partial signatures: got {}, need {}",
                input_idx,
                signature_count,
                c.min_signers
            ));
        }
    }

    // Now perform the actual aggregation using btc-server's finalize_signing logic
    println!("Starting signature aggregation...");

    // Load database to get public key package
    let db =
        database::Db::open(&c.db).map_err(|e| anyhow::anyhow!("Failed to open database: {}", e))?;

    let pk_package = db
        .get_public_key_package()
        .map_err(|e| anyhow::anyhow!("Failed to get public key package from database: {}", e))?
        .ok_or(anyhow::anyhow!("No public key package found in database"))?;

    // Extract signing packages from PSBT (reusing btc-server logic)
    let signing_packages = combined_psbt
        .signing_packages()
        .map_err(|e| anyhow::anyhow!("Failed to get signing packages from PSBT: {}", e))?;

    println!("Generated {} signing packages from PSBT", signing_packages.len());

    // Aggregate signatures for each input (based on btc-server's finalize_signing)
    for (index, psbt_input) in combined_psbt.inputs.iter_mut().enumerate() {
        let signing_package = signing_packages
            .get(index)
            .ok_or(anyhow::anyhow!("Missing signing package for input {}", index))?;

        let partial_sig = psbt_input.all_partial_signatures();
        let eth_address_tweak = psbt_input.eth_address();

        let signing_parameters = SigningParameters {
            tapscript_merkle_root: None,
            additional_tweak: eth_address_tweak.map(|e| e.to_vec()),
        };

        println!("Aggregating signatures for input {}", index);
        println!("  Partial signatures: {}", partial_sig.len());

        // Perform FROST aggregation (core btc-server logic)
        let agg_sig = frost::aggregate_with_tweak(
            signing_package,
            &partial_sig,
            &pk_package,
            &signing_parameters,
        )
        .map_err(|e| {
            anyhow::anyhow!("Failed to aggregate signatures for input {}: {}", index, e)
        })?;

        // Verify aggregated signature (btc-server validation)
        let effective_key = pk_package.clone().tweak(&signing_parameters);
        effective_key.verifying_key().verify(signing_package.message(), &agg_sig).map_err(|e| {
            anyhow::anyhow!("Signature verification failed for input {}: {}", index, e)
        })?;

        // Convert to Bitcoin format and finalize PSBT input (btc-server logic)
        let secp_sig = bitcoin::secp256k1::schnorr::Signature::from_slice(&agg_sig.serialize()?)
            .map_err(|e| {
                anyhow::anyhow!("Failed to convert signature for input {}: {}", index, e)
            })?;

        let hash_ty = bitcoin::sighash::TapSighashType::Default;
        let sighash_type = bitcoin::psbt::PsbtSighashType::from(hash_ty);
        psbt_input.sighash_type = Some(sighash_type);
        psbt_input.tap_key_sig =
            Some(bitcoin::taproot::Signature { signature: secp_sig, sighash_type: hash_ty });

        println!("✅ Input {} signature aggregated and verified", index);
    }

    // Clone PSBT for debug output before extracting transaction
    let psbt_for_debug = combined_psbt.clone();

    // Extract the final signed transaction
    let final_tx = combined_psbt
        .extract_tx()
        .map_err(|e| anyhow::anyhow!("Failed to extract final transaction: {}", e))?;

    let tx_hex = bitcoin::consensus::encode::serialize_hex(&final_tx);

    // Save to output file
    std::fs::write(&c.output_file, &tx_hex)
        .map_err(|e| anyhow::anyhow!("Failed to write transaction hex to file: {}", e))?;

    println!("🎉 Transaction finalization complete!");
    println!("   - Transaction hex saved to: {}", c.output_file.display());
    println!("   - Transaction ID: {}", final_tx.compute_txid());
    println!(
        "   - Transaction size: {} bytes",
        bitcoin::consensus::encode::serialize(&final_tx).len()
    );
    println!("   - Ready for broadcast!");

    // Save debug finalized PSBT
    let debug_psbt_filename = c.output_file.with_extension("psbt.json");
    let psbt_json = serde_json::to_string_pretty(&psbt_for_debug)
        .map_err(|e| anyhow::anyhow!("Failed to serialize debug PSBT: {}", e))?;

    std::fs::write(&debug_psbt_filename, psbt_json)
        .map_err(|e| anyhow::anyhow!("Failed to write debug PSBT: {}", e))?;

    println!("📋 Debug finalized PSBT saved to: {}", debug_psbt_filename.display());

    Ok(())
}

pub async fn handle_add_dummy_utxos(c: &AddDummyUtxosConfig) -> anyhow::Result<(), anyhow::Error> {
    let db = database::Db::open(&c.db).expect("failed to open db");
    add_dummy_utxos_to_db(&db).expect("failed to add dummy utxos to db");
    println!("dummy utxos added to db");
    Ok(())
}

pub fn dummy_utxos() -> Result<Vec<bitcoincore_rpc::json::Utxo>, anyhow::Error> {
    let json_data = r#"[{
  "txid": "d8b268a579ffbc5e425d69ef5f7e0f1c8db8c73b6b13b6f5a06caf4788129705",
  "vout": 1,
  "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
  "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
  "amount": 0.00180655,
  "height": 903664
},
{
  "txid": "2792d9c79713b7b3d2c1d0267ec567a9e05f79b18355c782f379b35ca08bd50d",
  "vout": 1,
  "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
  "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
  "amount": 0.01088904,
  "height": 904468
},
{
  "txid": "365e926b53fd9c01bda2d44b4ce2fd04eb97c63fffc732c8813cb7f0e625c40e",
  "vout": 2,
  "scriptPubKey": "5120f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8",
  "desc": "rawtr(f1de953e2a8b167981e9aaae3f02856dd491c431e0a37534a3859a18096feae8)#0wq8cgc9",
  "amount": 0.00146289,
  "height": 904173
}
]"#;

    // Parse the JSON into a Vec<Utxo>
    let utxos: Vec<bitcoincore_rpc::json::Utxo> = serde_json::from_str(json_data)
        .map_err(|e| anyhow::anyhow!("Failed to parse JSON: {}", e))?;

    // Print the results
    println!("Parsed {} UTXOs", utxos.len());

    Ok(utxos)
}

// THIS IS PURELY TO HELP WITH TESTING AND SHOULD NOT BE MERGED INTO THE FINAL CODE
pub fn add_dummy_utxos_to_db(db: &database::Db) -> Result<(), anyhow::Error> {
    // Use the above code as a reference for how to convert the dummy utxos to database::Utxo
    let utxos = dummy_utxos()?;
    let utxo_refs: Vec<database::Utxo> = utxos
        .iter()
        .map(|u| {
            database::Utxo::new(
                OutPoint::new(u.txid, u.vout),
                TxOut { value: u.amount, script_pubkey: u.script_pub_key.clone() },
                None,
                Some(UtxoVersion::V1),
            )
        })
        .collect();

    let utxo_refs_borrowed: Vec<&database::Utxo> = utxo_refs.iter().collect();
    db.store_utxos(&utxo_refs_borrowed).expect("failed to store utxos");
    Ok(())
}
