use clap::Args;
use reth_network::frost::manager::FrostConfig;

/// Parameters to configure Frost.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[clap(next_help_heading = "Frost")]

pub struct FrostArgs {
    /// Coordinator

    /// Min frost signers
    ///
    /// The minimum number required for frost signing.
    #[arg(long = "frost.min_signers", name = "frost.min_signers", value_name = "MIN_SIGNERS")]
    pub min_signers: u16,

    /// Max frost signers
    ///
    /// The maximum number required for frost signing.
    #[arg(long = "frost.max_signers", name = "frost.max_signers", value_name = "MAX_SIGNERS")]
    pub max_signers: u16,
}

impl From<FrostArgs> for FrostConfig {
    fn from(args: FrostArgs) -> Self {
        FrostConfig {
            authority_index: 0,
            total_authorities: 0,
            max_signers: args.max_signers,
            min_signers: args.min_signers,
        }
    }
}
