//! Unwinding a certain block range

use botanix_cli_args::chain::get_chain_from_federation_config;
use clap::{Parser, Subcommand};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_config::Config;
use reth_consensus::Consensus;
use reth_db::{init_db, DatabaseEnv};
use reth_db_api::database::Database;
use reth_downloaders::{bodies::noop::NoopBodiesDownloader, headers::noop::NoopHeaderDownloader};
use reth_evm::noop::NoopBlockExecutorProvider;
use reth_exex::ExExManagerHandle;
use reth_node_core::args::{DatabaseArgs, DatadirArgs, NetworkArgs};
use reth_primitives::{BlockHashOrNumber, BlockNumber, B256};
use reth_provider::{
    providers::StaticFileProvider, BlockExecutionWriter, BlockNumReader, ChainSpecProvider,
    FinalizedBlockReader, FinalizedBlockWriter, ProviderFactory, StaticFileProviderFactory,
};
use reth_prune::PruneModes;
use reth_stages::{
    sets::{DefaultStages, OfflineStages},
    stages::ExecutionStage,
    ExecutionStageThresholds, Pipeline, StageSet,
};
use reth_static_file::StaticFileProducer;
use std::{ops::RangeInclusive, path::PathBuf, sync::Arc};
use tokio::sync::watch;
use tracing::info;

/// `reth stage unwind` command
#[derive(Debug, Parser)]
pub struct Command {
    /// Parameters for datadir configuration
    #[command(flatten)]
    datadir: DatadirArgs,

    /// All database related arguments
    #[command(flatten)]
    db: DatabaseArgs,

    /// The path to the configuration file for the federation setup.
    #[arg(long, verbatim_doc_comment)]
    federation_config_path: PathBuf,

    /// All networking related arguments
    #[command(flatten)]
    network: NetworkArgs,

    /// Indicates whether we are running in testnet or not.
    #[arg(long, value_name = "IS_TESTNET")]
    pub is_testnet: bool,

    /// If this is enabled, then all stages except headers, bodies, and sender recovery will be
    /// unwound.
    #[arg(long)]
    offline: bool,

    #[command(subcommand)]
    command: Subcommands,
}

impl Command {
    /// Execute `db stage unwind` command
    pub async fn execute(self) -> eyre::Result<()> {
        // Load reth config which is a bit different than cli config

        // get the botanix chain spec
        let chain = get_chain_from_federation_config(
            self.federation_config_path.clone().to_str().expect("federation config path to exist"),
            self.is_testnet,
        )?;

        let data_dir =
            self.datadir.datadir.unwrap_or_chain_default(chain.chain, self.datadir.clone());

        let db_path = data_dir.db();

        let database = Arc::new(init_db(db_path.clone(), self.db.database_args())?.with_metrics());

        let static_file_provider = StaticFileProvider::read_write(data_dir.static_files())?;
        let provider_factory = ProviderFactory::<Arc<DatabaseEnv>>::new(
            database,
            Arc::new(chain),
            static_file_provider,
        );

        // TODO: We might need to configure it
        let config = Config::default();

        let range = self.command.unwind_range(provider_factory.clone())?;
        if *range.start() == 0 {
            eyre::bail!("Cannot unwind genesis block")
        }

        let highest_static_file_block = provider_factory
            .static_file_provider()
            .get_highest_static_files()
            .max()
            .filter(|highest_static_file_block| highest_static_file_block >= range.start());

        // Execute a pipeline unwind if the start of the range overlaps the existing static
        // files. If that's the case, then copy all available data from MDBX to static files, and
        // only then, proceed with the unwind.
        //
        // We also execute a pipeline unwind if `offline` is specified, because we need to only
        // unwind the data associated with offline stages.
        if highest_static_file_block.is_some() || self.offline {
            if self.offline {
                info!(target: "reth::cli", "Performing an unwind for offline-only data!");
            }

            if let Some(highest_static_file_block) = highest_static_file_block {
                info!(target: "reth::cli", ?range, ?highest_static_file_block, "Executing a pipeline unwind.");
            } else {
                info!(target: "reth::cli", ?range, "Executing a pipeline unwind.");
            }

            // This will build an offline-only pipeline if the `offline` flag is enabled
            let mut pipeline = self.build_pipeline(config, provider_factory)?;

            // Move all applicable data from database to static files.
            pipeline.move_to_static_files()?;

            pipeline.unwind((*range.start()).saturating_sub(1), None)?;
        } else {
            info!(target: "reth::cli", ?range, "Executing a database unwind.");
            let provider = provider_factory.provider_rw()?;

            let _ = provider
                .take_block_and_execution_range(range.clone())
                .map_err(|err| eyre::eyre!("Transaction error on unwind: {err}"))?;

            // update finalized block if needed
            let last_saved_finalized_block_number = provider.last_finalized_block_number()?;
            let range_min =
                range.clone().min().ok_or(eyre::eyre!("Could not fetch lower range end"))?;
            if last_saved_finalized_block_number.is_none() ||
                Some(range_min) < last_saved_finalized_block_number
            {
                provider.save_finalized_block_number(BlockNumber::from(range_min))?;
            }

            provider.commit()?;
        }

        info!(target: "reth::cli", range=?range.clone(), count=range.count(), "Unwound blocks");

        Ok(())
    }

    fn build_pipeline<DB: Database + 'static>(
        self,
        config: Config,
        provider_factory: ProviderFactory<Arc<DB>>,
    ) -> Result<Pipeline<Arc<DB>>, eyre::Error> {
        let consensus: Arc<dyn Consensus> =
            Arc::new(EthBeaconConsensus::new(provider_factory.chain_spec()));
        let stage_conf = &config.stages;
        let prune_modes = config.prune.clone().map(|prune| prune.segments).unwrap_or_default();

        let (tip_tx, tip_rx) = watch::channel(B256::ZERO);

        // Unwinding does not require a valid executor
        let executor = NoopBlockExecutorProvider::default();

        let builder = if self.offline {
            Pipeline::builder().add_stages(
                OfflineStages::new(executor, config.stages, PruneModes::default())
                    .builder()
                    .disable(reth_stages::StageId::SenderRecovery),
            )
        } else {
            Pipeline::builder().with_tip_sender(tip_tx).add_stages(
                DefaultStages::new(
                    provider_factory.clone(),
                    tip_rx,
                    Arc::clone(&consensus),
                    NoopHeaderDownloader::default(),
                    NoopBodiesDownloader::default(),
                    executor.clone(),
                    stage_conf.clone(),
                    prune_modes.clone(),
                )
                .set(ExecutionStage::new(
                    executor,
                    ExecutionStageThresholds {
                        max_blocks: None,
                        max_changes: None,
                        max_cumulative_gas: None,
                        max_duration: None,
                    },
                    stage_conf.execution_external_clean_threshold(),
                    prune_modes,
                    ExExManagerHandle::empty(),
                )),
            )
        };

        let pipeline = builder.build(
            provider_factory.clone(),
            StaticFileProducer::new(provider_factory, PruneModes::default()),
        );
        Ok(pipeline)
    }
}

/// `reth stage unwind` subcommand
#[derive(Subcommand, Debug, Eq, PartialEq)]
enum Subcommands {
    /// Unwinds the database from the latest block, until the given block number or hash has been
    /// reached, that block is not included.
    #[command(name = "to-block")]
    ToBlock { target: BlockHashOrNumber },
    /// Unwinds the database from the latest block, until the given number of blocks have been
    /// reached.
    #[command(name = "num-blocks")]
    NumBlocks { amount: u64 },
}

impl Subcommands {
    /// Returns the block range to unwind.
    ///
    /// This returns an inclusive range: [target..=latest]
    fn unwind_range<DB: Database>(
        &self,
        factory: ProviderFactory<DB>,
    ) -> eyre::Result<RangeInclusive<u64>> {
        let provider = factory.provider()?;
        let last = provider.last_block_number()?;
        let target = match self {
            Self::ToBlock { target } => match target {
                BlockHashOrNumber::Hash(hash) => provider
                    .block_number(*hash)?
                    .ok_or_else(|| eyre::eyre!("Block hash not found in database: {hash:?}"))?,
                BlockHashOrNumber::Number(num) => *num,
            },
            Self::NumBlocks { amount } => last.saturating_sub(*amount),
        } + 1;
        if target > last {
            eyre::bail!("Target block number is higher than the latest block number")
        }
        Ok(target..=last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unwind() {
        let cmd = Command::parse_from(["reth", "--datadir", "dir", "to-block", "100"]);
        assert_eq!(cmd.command, Subcommands::ToBlock { target: BlockHashOrNumber::Number(100) });

        let cmd = Command::parse_from(["reth", "--datadir", "dir", "num-blocks", "100"]);
        assert_eq!(cmd.command, Subcommands::NumBlocks { amount: 100 });
    }
}
