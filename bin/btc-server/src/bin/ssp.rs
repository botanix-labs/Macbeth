use anyhow::Result;
use bitcoin::{
    secp256k1::{self, Message},
    taproot::LeafVersion,
    FeeRate, OutPoint, Psbt, TapLeafHash, TxOut,
};
use bitcoin_hashes::Hash;
use btcserverlib::{
    database::{version::UtxoVersion, Utxo},
    pegout_id::PegoutId,
    wallet::{
        address::{
            generate_ssp_script, generate_taproot_change_scriptpubkey, generate_taproot_spend_info,
        },
        coin_selection::coin_selection,
        psbt::calculate_scriptpath_sighash,
    },
};
use miniscript::{psbt::PsbtExt, ToPublicKey};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    str::FromStr,
};

const KEY_OUTPUT_PATH: &str = "./ssp_prv_key.hex";

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "SSP CLI")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,
}
#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// SSP key pair gen
    KeyGen(KeyGen),
    /// Generate taproot merkel root given the public keys
    GenerateTaprootMerkleRoot(GenerateMerkleRoot),
    /// Generate PSBT spend
    GeneratePsbtSpend(SSPSpendPsbt),
    /// Sign a PSBT
    SignPsbt(SignPsbt),
    /// Combine PSBTs
    CombinePsbt(CombinePsbt),
}

#[derive(Parser, Debug)]
pub(crate) struct KeyGen {
    #[arg(short, long)]
    pub secret_key_path: Option<String>,
}

#[derive(Parser, Debug)]
pub(crate) struct GenerateMerkleRoot {
    /// Public keys
    #[arg(short, long)]
    pub public_keys: Vec<String>,

    /// Aggregate public key
    #[arg(short, long)]
    pub aggregate_public_key: String,
}

#[derive(Parser, Debug)]
pub(crate) struct SSPSpendPsbt {
    /// SSP public key
    #[arg(short, long)]
    pub aggregate_public_key: String,

    /// Amount to spend
    #[arg(short, long)]
    pub amount: u64,

    /// Output address
    #[arg(short, long)]
    pub output_address: String,

    /// Inputs to spend
    /// Formatted as <txid>:<vout>
    #[arg(short, long)]
    pub inputs_to_spend: Vec<String>,
}

#[derive(Parser, Debug)]
pub(crate) struct SignPsbt {
    /// Secret key path
    #[arg(short, long)]
    pub secret_key_path: String,
    /// PSBT path
    #[arg(short, long)]
    pub psbt_path: String,
    /// Public keys
    #[arg(short, long)]
    pub pks: Vec<String>,
}

#[derive(Parser, Debug)]
pub(crate) struct CombinePsbt {
    /// PSBT paths
    #[arg(short, long)]
    pub psbt_paths: Vec<String>,
}

fn generate_key(output_path: &str) {
    // TODO: use a global secp context
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let key = secp256k1::Keypair::new(&secp, &mut rand::thread_rng());
    let key_bytes = key.secret_bytes();
    let key_hex = hex::encode(key_bytes);
    // print pk to stdout
    println!("Your SSP public key is: {}", key.public_key());
    fs::write(output_path, key_hex).expect("Failed to write key to file");
}

fn generate_taproot_merkel_root(
    public_keys: Vec<String>,
    aggregate_public_key: String,
) -> Result<()> {
    let agg_pk = bitcoin::PublicKey::from_str(&aggregate_public_key)
        .map_err(|_| anyhow::anyhow!("Invalid aggregate public key"))?;
    // First lets parse the public keys
    let public_keys = public_keys
        .iter()
        .map(|pk| {
            let pk = bitcoin::PublicKey::from_str(pk)
                .map_err(|_| anyhow::anyhow!("Invalid public key"))?;
            Ok(pk)
        })
        .collect::<Result<Vec<_>>>()?;
    let taproot_spend_info = generate_taproot_spend_info(public_keys, agg_pk);

    let merkle_root =
        taproot_spend_info.merkle_root().ok_or(anyhow::anyhow!("Couldn't get merkle root"))?;
    println!("Merkle root: {}", hex::encode(merkle_root));

    Ok(())
}

fn generate_psbt_spend(
    agg_pk: bitcoin::secp256k1::PublicKey,
    amount: bitcoin::Amount,
    spk: bitcoin::ScriptBuf,
    inputs_to_spend: Vec<OutPoint>,
) -> Result<()> {
    // use dummy pegout id
    let outputs =
        vec![(TxOut { value: amount, script_pubkey: spk.clone() }, PegoutId::new([0; 32], 0))];
    // TODO hardcoded fee rate
    let fee_rate = FeeRate::BROADCAST_MIN;
    let change_script = generate_taproot_change_scriptpubkey(&agg_pk);
    let mut available_utxos = HashMap::new();
    for input in inputs_to_spend.iter() {
        available_utxos.insert(
            input.clone(),
            Utxo {
                outpoint: input.clone(),
                // Note: using a dummy Output here
                output: TxOut { value: amount, script_pubkey: spk.clone() },
                eth_address: None,
                version: UtxoVersion::V1 as u32,
            },
        );
    }
    let psbt = coin_selection(available_utxos, HashMap::new(), outputs, fee_rate, change_script)?;
    // write ssp to file
    fs::write("./ssp.psbt", psbt.to_string())?;
    Ok(())
}

fn sign_psbt(
    secret_key_path: String,
    psbt_path: String,
    pks: Vec<bitcoin::PublicKey>,
) -> Result<()> {
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let secret_key = fs::read_to_string(secret_key_path)?;
    let secret_key = bitcoin::secp256k1::SecretKey::from_str(&secret_key)?;
    let key_pair = bitcoin::secp256k1::Keypair::from_secret_key(&secp, &secret_key);
    let pk = secret_key.public_key(&secp);
    let mut psbt = Psbt::from_str(&fs::read_to_string(psbt_path)?)?;
    let script = generate_ssp_script(pks);
    let tapleaf_hash = TapLeafHash::from_script(&script, LeafVersion::TapScript);

    let psbt_clone = psbt.clone();
    for (i, input) in psbt.inputs.iter_mut().enumerate() {
        let sig_hash = calculate_scriptpath_sighash(&psbt_clone, i, tapleaf_hash)?;
        let message = Message::from_digest(
            sig_hash.to_raw_hash().to_byte_array().to_vec().try_into().unwrap(),
        );
        let signature = secp.sign_schnorr(&message, &key_pair);
        let taproot_sig =
            bitcoin::taproot::Signature { sighash_type: bitcoin::TapSighashType::All, signature };
        let mut sigs = BTreeMap::new();
        sigs.insert((pk.to_x_only_pubkey(), tapleaf_hash), taproot_sig);
        input.tap_script_sigs = sigs;
    }
    fs::write("./signed_ssp.psbt", psbt.to_string())?;
    Ok(())
}

fn combine_psbt(psbt_paths: Vec<String>) -> Result<()> {
    // TODO: use a global secp context
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let psbts = psbt_paths
        .iter()
        .map(|path| Psbt::from_str(&fs::read_to_string(path).unwrap()).unwrap())
        .collect::<Vec<_>>();
    let mut acc = psbts[0].clone();
    for psbt in psbts.iter().skip(1) {
        acc.combine(psbt.clone())?;
    }

    acc.finalize_mut(&secp).map_err(|_| anyhow::anyhow!("Failed to finalize PSBT"))?;
    fs::write("./combined_ssp.psbt", acc.to_string())?;

    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::KeyGen(config) => {
            generate_key(&config.secret_key_path.unwrap_or(KEY_OUTPUT_PATH.to_string()));
        }
        Commands::GenerateTaprootMerkleRoot(config) => {
            generate_taproot_merkel_root(config.public_keys, config.aggregate_public_key)?;
        }
        Commands::GeneratePsbtSpend(config) => {
            let inputs_to_spend: Vec<OutPoint> =
                config.inputs_to_spend.iter().map(|s| OutPoint::from_str(s).unwrap()).collect();
            let output_spk = bitcoin::address::Address::from_str(&config.output_address)?
                .assume_checked()
                .script_pubkey();
            let amount = bitcoin::Amount::from_sat(config.amount);
            let agg_pk = bitcoin::secp256k1::PublicKey::from_str(&config.aggregate_public_key)?;
            generate_psbt_spend(agg_pk, amount, output_spk, inputs_to_spend)?;
        }
        Commands::SignPsbt(config) => {
            let pks =
                config.pks.iter().map(|pk| bitcoin::PublicKey::from_str(pk).unwrap()).collect();
            sign_psbt(config.secret_key_path, config.psbt_path, pks)?;
        }
        Commands::CombinePsbt(config) => {
            combine_psbt(config.psbt_paths)?;
        }
    }

    Ok(())
}
