//! `reth stage` command

use clap::{Parser, Subcommand};

pub mod unwind;

/// `reth stage` command
#[derive(Debug, Parser)]
pub struct Command {
    #[command(subcommand)]
    command: Subcommands,
}

/// `reth stage` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands {
    /// Unwinds a certain block range, deleting it from the database.
    Unwind(unwind::Command),
}

impl Command {
    /// Execute `stage` command
    pub async fn execute(self) -> eyre::Result<()> {
        match self.command {
            Subcommands::Unwind(command) => command.execute().await,
        }
    }
}
