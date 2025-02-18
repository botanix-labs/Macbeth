use anyhow::Result;
use clap::Parser;
#[derive(Parser, Debug)]
#[command(name = "Botanix Up")]
pub(crate) struct Cli {
    /// Config.toml path (default path is home directory)
    #[arg(short = 'o', long)]
    pub output_path: Option<String>,

    /// numbers of nodes
    #[arg(short = 'n', long)]
    pub(crate) num_nodes: u16,

    /// multisig min signers
    #[arg(short = 'm', long)]
    pub(crate) multisig_min_signers: u16,

    /// multisig max signers
    #[arg(short = 't', long)]
    pub(crate) multisig_max_signers: u16,
}

impl Cli {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.num_nodes == 0 {
            return Err(anyhow::anyhow!("Number of nodes must be greater than 0"));
        }

        if self.multisig_max_signers == 0 {
            return Err(anyhow::anyhow!("Max signers must be greater than 0"));
        }

        if self.multisig_min_signers == 0 {
            return Err(anyhow::anyhow!("Min signers must be greater than 0"));
        }

        if self.multisig_min_signers > self.num_nodes || self.multisig_max_signers > self.num_nodes
        {
            return Err(anyhow::anyhow!(
                "Min signers and max signers must be less than or equal to the number of nodes"
            ));
        }

        if self.multisig_max_signers < self.multisig_min_signers {
            return Err(anyhow::anyhow!(
                "Max signers must be greater than or equal to the min signers"
            ));
        }
        Ok(())
    }
}
