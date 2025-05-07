//! This binary is meant to clean different entities from the botanix db.

mod cli;
use anyhow::Result as AnyResult;
use clap::Parser;
use cli::Cli;
use reth_chainspec::BOTANIX_TESTNET;
use reth_db::{
    mdbx::{DatabaseArguments, MaxReadTransactionDuration},
    models::ClientVersion,
    open_db, DatabaseEnv,
};
use reth_provider::{errors::db::LogLevel, providers::StaticFileProvider, ProviderFactory};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// db scopes
pub mod scopes;

#[tokio::main]
async fn main() -> AnyResult<()> {
    let cli = Cli::parse();
    // Basic sanity checks
    cli.validate()?;
    let db_path = PathBuf::from(cli.db_path);
    tracing::info!("Db path: {:?}", db_path);
    tracing::info!("Entity to truncate: {:?}", cli.entity);

    // database provider
    let db_args = DatabaseArguments::new(ClientVersion::default())
        .with_exclusive(Some(true))
        .with_log_level(Some(LogLevel::Debug))
        .with_max_read_transaction_duration(Some(MaxReadTransactionDuration::Unbounded));
    let db_dir = Path::new(&db_path).join("db");
    let node_config = BOTANIX_TESTNET.clone();
    let static_files_dir = Path::new(&db_path).join("static_files");

    tracing::info!(target: "db_cleaner::cli", path = ?db_dir, "Opening database ...");

    let db = loop {
        match open_db(&db_dir, db_args.clone()) {
            Ok(db) => {
                break db;
            }
            Err(e) => {
                tracing::error!(target: "db_cleaner::cli", path = ?db_path, "Opening database failed - retrying. Error = {e:?}");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        }
    };
    tracing::info!(target: "db_cleaner::cli", path = ?db_path, "Database successfully opened!");
    let database = Arc::new(db);

    let static_file_provider = StaticFileProvider::read_write(static_files_dir)?;
    let provider_factory =
        ProviderFactory::<Arc<DatabaseEnv>>::new(database, node_config, static_file_provider);

    match cli.entity {
        cli::Entity::Snapshots => {
            tracing::info!(target: "db_cleaner::cli", "Truncating all snapshot-related db tables ...");
            scopes::snapshots::truncate(&provider_factory)?;
        }
    }

    Ok(())
}
