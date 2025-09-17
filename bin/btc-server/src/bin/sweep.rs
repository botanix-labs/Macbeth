use std::{fs::File, io::Write, path::PathBuf};

use base64::prelude::*;
use bitcoin::{consensus::encode::serialize_hex, psbt::Psbt, Amount, FeeRate, OutPoint, TxOut};
use btcserverlib::{
    database,
    database::version::UtxoVersion,
    wallet::{psbt::PsbtInputExt, util::calculate_signed_tx_weight},
};
use clap::Parser;
use frost_secp256k1_tr as frost;
use frost_secp256k1_tr::round1::{SigningCommitments, SigningNonces};
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
    #[command(name = "make-sweep-psbt")]
    MakeSweepPsbt(MakeSweepPsbtConfig),
    #[command(name = "frost-round-1")]
    FrostRound1(FrostRound1Config),
    #[command(name = "frost-round-2")]
    FrostRound2(FrostRound2Config),
    #[command(name = "add-dummy-utxos")]
    AddDummyUtxos(AddDummyUtxosConfig),
}

#[derive(Clone, Debug, Parser)]
pub struct MakeSweepPsbtConfig {
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
pub struct FrostRound1Config {
    #[arg(long)]
    pub input_json: PathBuf,
    #[arg(long)]
    pub db: PathBuf,
    #[arg(long)]
    pub identifier: u16, // TODO: get this from the same config as btc-server
}

#[derive(Clone, Debug, Parser)]
pub struct FrostRound2Config {}

#[derive(Clone, Debug, Parser)]
pub struct AddDummyUtxosConfig {
    #[arg(long)]
    pub db: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct SigningPackage {
    psbt_hex: String,
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
        Commands::MakeSweepPsbt(config) => handle_make_sweep_psbt(&config).await?,
        Commands::FrostRound1(config) => handle_frost_round_1(&config).await?,
        Commands::FrostRound2(config) => handle_frost_round_2(&config).await?,
        Commands::AddDummyUtxos(config) => handle_add_dummy_utxos(&config).await?,
    }

    Ok(())
}

pub async fn handle_make_sweep_psbt(c: &MakeSweepPsbtConfig) -> anyhow::Result<(), anyhow::Error> {
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

    // TODO: store the psbt in a new json file
    let psbt_json = serde_json::to_string(&psbt).expect("failed to serialize psbt");
    let mut file = File::create("psbt.json").expect("failed to create file");
    file.write_all(psbt_json.as_bytes()).expect("failed to write to file");

    println!("psbt: {:?}", psbt);

    println!("psbt tx hex = {}", serialize_hex(&psbt.unsigned_tx));

    // create random signing session id
    let mut signing_session_id = [0u8; SIGNING_SESSION_ID_SIZE];
    rand::thread_rng().fill_bytes(&mut signing_session_id);

    // create a dummy identifier (since it doesn't matter for this use case)
    let dummy_identifier = vec![1u8; DUMMY_IDENTIFIER_SIZE]; // 33 bytes for a compressed public key

    // create the serializable SigningPackage structure
    let signing_package = SigningPackage {
        psbt_hex: psbt.serialize_hex(),
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

pub async fn handle_frost_round_1(c: &FrostRound1Config) -> anyhow::Result<(), anyhow::Error> {
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
    let mut psbt = Psbt::deserialize(&hex::decode(&input_package.psbt_hex)?)
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
        psbt_hex: psbt.serialize_hex(),
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

pub async fn handle_frost_round_2(_c: &FrostRound2Config) -> anyhow::Result<(), anyhow::Error> {
    println!("frost round 2");
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
