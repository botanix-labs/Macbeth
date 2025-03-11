use anyhow::Result;
use bitcoin::{secp256k1, taproot::LeafVersion};
use btcserverlib::wallet::address::generate_ssp_script;
use log::info;
use miniscript::ToPublicKey;
use std::{fs, str::FromStr};

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
    GeneratePsbtSpend,
    /// Sign a PSBT
    SignPsbt,
    /// Combine PSBTs
    CombinePsbt,
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
    // TODO: use a global secp context
    let secp = bitcoin::secp256k1::Secp256k1::new();
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
    let script = generate_ssp_script(public_keys);
    let builder = bitcoin::taproot::TaprootBuilder::new()
        .add_leaf(0u8, script.clone())
        .expect("Couldn't add ssp leaf");

    let taproot_spend_info =
        builder.finalize(&secp, agg_pk.to_x_only_pubkey()).expect("Couldn't finalize taproot");
    let control_block = taproot_spend_info
        .control_block(&(script, LeafVersion::TapScript))
        .expect("Couldn't get control block");
    // TODO do we need to save the control block as well

    let merkle_root =
        taproot_spend_info.merkle_root().ok_or(anyhow::anyhow!("Couldn't get merkle root"))?;
    println!("Merkle root: {}", hex::encode(merkle_root));

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
        Commands::GeneratePsbtSpend => {
            unimplemented!()
        }
        Commands::SignPsbt => {
            unimplemented!()
        }
        Commands::CombinePsbt => {
            unimplemented!()
        }
    }

    Ok(())
}
