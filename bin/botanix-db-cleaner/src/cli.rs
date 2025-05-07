use anyhow::Result;
use clap::{Parser, ValueEnum};
use std::path::Path;

#[derive(ValueEnum, Clone, Debug)]
#[clap(rename_all = "kebab_case")]
pub(crate) enum Entity {
    Snapshots,
}

#[derive(Parser, Debug)]
#[command(name = "botanix-db-clean")]
pub(crate) struct Cli {
    /// db path
    #[arg(short = 'd', long)]
    pub db_path: String,

    /// entity to remove
    #[arg(short = 'e', long)]
    pub(crate) entity: Entity,
}

impl Cli {
    pub(crate) fn validate(&self) -> Result<()> {
        // Check that db_path exists and is a directory
        let path = Path::new(&self.db_path);

        if !path.exists() {
            return Err(anyhow::anyhow!("Database path '{}' does not exist", self.db_path));
        }

        if !path.is_dir() {
            return Err(anyhow::anyhow!("Database path '{}' is not a directory", self.db_path));
        }

        match &self.entity {
            Entity::Snapshots => {
                // currently allowed
            }
            _ => return Err(anyhow::anyhow!("Unsupported entity")),
        }

        Ok(())
    }
}
