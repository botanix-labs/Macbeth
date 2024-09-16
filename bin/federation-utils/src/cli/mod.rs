use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "Wallet CLI")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// Sets up the wallet
    Setup(Setup),
    /// Generates a key
    GenerateKey(GenerateKey),
    /// Gets the balance
    GetBalance(GetBalance),
    /// Sweeps the balance
    SweepBalance(SweepBalance),
}

/// Command for setting up the wallet config
#[derive(Parser, Debug)]
pub(crate) struct Setup {
    /// Chain ID
    #[arg(long)]
    pub(crate) chain_id: u64,

    /// Path to the secret key output file
    #[arg(long)]
    pub(crate) secret_key_destination: Option<String>,

    /// Path to the wallet config file (optional)
    #[arg(long)]
    pub(crate) provider_url: Option<String>,

    /// Path to the wallet config file (optional)
    #[arg(long)]
    pub(crate) config_path: Option<String>,

    /// Address to receive funds (optional)
    #[arg(long)]
    pub(crate) receiver_address: Option<String>,
}

#[derive(Parser, Debug)]
pub(crate) struct GenerateKey {}

#[derive(Parser, Debug)]
pub(crate) struct GetBalance {}

#[derive(Parser, Debug)]
pub(crate) struct SweepBalance {}
