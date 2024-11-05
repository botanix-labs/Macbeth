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
    /// Init create config.toml in home directory.
    Init,
    /// Get balance of an address.
    GetBalance(GetBalance),
    /// Sweep the balance to another address.
    SweepBalance(SweepBalance),
    /// Get Transaction Details.
    GetTransaction(GetTransaction),
}

#[derive(Parser, Debug)]
pub(crate) struct GetBalance {
    ///`secret_key_path`
    #[arg(short, long)]
    pub secret_key_path: Option<String>,
}

#[derive(Parser, Debug)]
pub(crate) struct GetTransaction {
    /// Transaction hash
    pub tx_hash: String,
}

#[derive(Parser, Debug)]
pub(crate) struct SweepBalance {
    #[arg(short, long)]
    pub secret_key_path: Option<String>,

    #[arg(short, long)]
    pub receiver_address: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn test_cli_init_command() {
        let args = vec!["wallet_cli", "init"];
        let cli = Cli::parse_from(args);

        if let Commands::Init = cli.command {
            assert!(true);
        } else {
            panic!("Expected Init command.");
        }
    }

    #[test]
    fn test_cli_get_balance_with_secret_key_path() {
        let args = vec!["wallet_cli", "get-balance", "--secret-key-path", "0x1234567890abcdef"];
        let cli = Cli::parse_from(args);

        if let Commands::GetBalance(get_balance) = cli.command {
            assert_eq!(get_balance.secret_key_path.unwrap(), "0x1234567890abcdef");
        } else {
            panic!("Expected GetBalance command.");
        }
    }

    #[test]
    fn test_cli_get_balance_without_secret_key_path() {
        let args = vec!["wallet_cli", "get-balance"];
        let cli = Cli::parse_from(args);

        if let Commands::GetBalance(get_balance) = cli.command {
            assert!(get_balance.secret_key_path.is_none());
        } else {
            panic!("Expected GetBalance command.");
        }
    }

    #[test]
    fn test_cli_get_transaction_with_tx_hash() {
        let args = vec!["wallet_cli", "get-transaction", "0xabc123"];
        let cli = Cli::parse_from(args);

        if let Commands::GetTransaction(get_transaction) = cli.command {
            assert_eq!(get_transaction.tx_hash, "0xabc123");
        } else {
            panic!("Expected GetTransaction command.");
        }
    }

    #[test]
    fn test_cli_sweep_balance_without_optional_params() {
        let args = vec!["wallet_cli", "sweep-balance"];
        let cli = Cli::parse_from(args);

        if let Commands::SweepBalance(sweep_balance) = cli.command {
            assert!(sweep_balance.secret_key_path.is_none());
            assert!(sweep_balance.receiver_address.is_none());
        } else {
            panic!("Expected SweepBalance command.");
        }
    }

    #[test]
    fn test_cli_with_config_path_and_chain_id() {
        let args = vec![
            "wallet_cli",
            "-p",
            "/path/to/config.toml",
            "--chain-id",
            "123",
            "--provider-url",
            "http://localhost:8545",
            "init",
        ];
        let cli = Cli::parse_from(args);

        assert_eq!(cli.config_path.unwrap(), "/path/to/config.toml");
        assert_eq!(cli.chain_id.unwrap(), 123);
        assert_eq!(cli.provider_url.unwrap(), "http://localhost:8545");
    }
}
