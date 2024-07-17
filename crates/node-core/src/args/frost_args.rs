use clap::Args;

/// Default min signers
pub(crate) const DEFAULT_MIN_SIGNERS: u16 = 2;

/// Default max signers
pub(crate) const DEFAULT_MAX_SIGNERS: u16 = 2;

/// Parameters to configure Frost.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[clap(next_help_heading = "Frost")]
pub struct FrostArgs {
    /// Coordinator

    /// Min frost signers
    ///
    /// The minimum number required for frost signing.
    #[arg(default_value_t=DEFAULT_MIN_SIGNERS, long = "frost.min_signers", name = "frost.min_signers", value_name = "MIN_SIGNERS")]
    pub min_signers: u16,

    /// Max frost signers
    ///
    /// The maximum number required for frost signing.
    #[arg(default_value_t=DEFAULT_MAX_SIGNERS, long = "frost.max_signers", name = "frost.max_signers", value_name = "MAX_SIGNERS")]
    pub max_signers: u16,
}
