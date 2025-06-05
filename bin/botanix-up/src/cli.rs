use anyhow::Result;
use clap::Parser;
use resolve_path::PathResolveExt;
use std::env;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "Botanix Up")]
pub(crate) struct Cli {
    /// Config.toml path (default path is HOME/.botanix-local)
    #[arg(
        short = 'o',
        long,
        default_value = "~/.botanix-local",
        value_parser = resolve_path
    )]
    pub(crate) output_path: PathBuf,

    /// Docker compose project name prefix. The full name will be [prefix][NodeIndex]
    #[arg(long, default_value = "botanix", value_parser = validate_project_name_prefix)]
    pub(crate) project_name_prefix: String,

    /// Generate configs for non-docker environment (localhost networking)
    #[arg(long, default_value_t = false)]
    pub(crate) non_docker: bool,

    /// numbers of nodes
    #[arg(short = 'n', long)]
    pub(crate) num_nodes: u16,

    /// Multisig min signers. It equals to the number of nodes by default.
    #[arg(short = 'm', long)]
    multisig_min_signers: Option<u16>,

    /// Multisig max signers. It equals to the number of nodes by default.
    #[arg(short = 't', long)]
    multisig_max_signers: Option<u16>,

    /// Block fee recipient address. Default is the prefunded balance for testnet.
    #[arg(long, default_value = "0xF27a6Ea4a1d5f7341Da7EDAaa47C5C933b738f4F")]
    pub(crate) block_fee_recipient: String,
}

impl Cli {
    pub(crate) fn multisig_min_signers(&self) -> u16 {
        self.multisig_min_signers.unwrap_or(self.num_nodes)
    }

    pub(crate) fn multisig_max_signers(&self) -> u16 {
        self.multisig_max_signers.unwrap_or(self.num_nodes)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.num_nodes == 0 {
            return Err(anyhow::anyhow!("Number of nodes must be greater than 0"));
        }

        if self.multisig_max_signers() == 0 {
            return Err(anyhow::anyhow!("Max signers must be greater than 0"));
        }

        if self.multisig_min_signers() == 0 {
            return Err(anyhow::anyhow!("Min signers must be greater than 0"));
        }

        if self.multisig_min_signers() > self.num_nodes
            || self.multisig_max_signers() > self.num_nodes
        {
            return Err(anyhow::anyhow!(
                "Min signers and max signers must be less than or equal to the number of nodes"
            ));
        }

        if self.multisig_max_signers() < self.multisig_min_signers() {
            return Err(anyhow::anyhow!(
                "Max signers must be greater than or equal to the min signers"
            ));
        }

        if self.output_path.exists() {
            return Err(anyhow::anyhow!("Output path already exists: {:?}", self.output_path));
        }

        Ok(())
    }
}

fn resolve_path(s: &str) -> Result<PathBuf, String> {
    s.try_resolve().map(|path| path.into_owned()).map_err(|e| e.to_string())
}

fn validate_project_name_prefix(s: &str) -> Result<String, String> {
    if s.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(s.to_string())
    } else {
        Err("Project name prefix must contain only alphanumeric characters".to_string())
    }
}
