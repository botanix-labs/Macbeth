use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "Wallet CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Sets up the wallet
    Setup(Setup),
    /// Generates a key
    GenerateKey(GenerateKey),
    /// Gets the balance
    GetBalance(GetBalance),
    /// Sweeps the balance
    SweepBalance(SweepBalance),
}

/// Command for setting up the wallet
#[derive(Parser, Debug)]
pub struct Setup {
    /// Path to the wallet config file
    #[arg(long)]
    config_path: String,

    /// Path to the secret key output file
    #[arg(long)]
    secret_key_output_path: String,

    /// Chain ID
    #[arg(long)]
    chain_id: u64,

    /// Address to receive funds (optional)
    #[arg(long)]
    receiver_address: Option<String>,
}

#[derive(Parser, Debug)]
pub struct GenerateKey {}

#[derive(Parser, Debug)]
pub struct GetBalance {}

#[derive(Parser, Debug)]
pub struct SweepBalance {}
