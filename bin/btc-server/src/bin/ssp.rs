use bitcoin::secp256k1;
use log::info;
use std::fs;

const KEY_OUTPUT_PATH: &str = "./ssp_prv_key.hex";

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "Wallet CLI")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,

    /// Config.toml path (default path is home directory)
    #[arg(short = 'p', long)]
    pub config_path: Option<String>,

    /// Chain ID (default 3636, can be loaded from `config.toml`)
    #[arg(short, long)]
    pub(crate) chain_id: Option<u64>,

    /// Provider URL (defaults to `http://localhost:8545`, can be loaded from `config.toml`)
    #[arg(short = 'u', long)]
    pub(crate) provider_url: Option<String>,
}
#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// SSP key pair gen
    KeyGen(KeyGen),
}

#[derive(Parser, Debug)]
pub(crate) struct KeyGen {
    /// `secret_key_path`
    #[arg(short, long)]
    pub secret_key_path: Option<&str>,
}

fn generate_key(output_path: &str) {
    let key = secp256k1::Keypair::new_global(&mut rand::thread_rng());
    let key_bytes = key.secret_bytes();
    let key_hex = hex::encode(key_bytes);
    // print pk to stdout
    info!("Your SSP public key is: {}", key.public_key());
    fs::write(output_path, key_hex).expect("Failed to write key to file");
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::KeyGen(config) => {
            generate_key(kconfig.secret_key_path.unwrap_or(KEY_OUTPUT_PATH));
        }
    }
}
