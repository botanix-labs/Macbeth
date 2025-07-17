use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
};

use btc::Utxo;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod btc;
mod config;
mod error;

use btc::BtcClient;
use config::Config;
use error::{EmergencyToolError, Result};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manages UTXOs
    Utxo {
        #[command(subcommand)]
        command: UtxoCommands,
    },
    /// Constructs a PSBT that consolidates all UTXOs to a destination address (coordinator only)
    Sweep {
        /// The destination address to sweep all funds to
        #[clap(short, long)]
        destination: String,
        /// The path to save the PSBT file. Defaults to 'emergency_sweep.psbt'
        #[clap(short, long, default_value = "emergency_sweep.psbt")]
        output: PathBuf,
        /// Path to a JSON file containing manually curated UTXOs (skips consensus)
        #[clap(long)]
        utxo_file: Option<PathBuf>,
        /// Federation member exported UTXO files for consensus mode (comma-separated paths)
        #[clap(long, value_delimiter = ',')]
        member_files: Option<Vec<PathBuf>>,
        /// Consensus threshold as percentage (default: 67 for >2/3 supermajority)
        #[clap(long, default_value = "67")]
        consensus_threshold: u8,
        /// Path to save excluded UTXOs report (only used in consensus mode)
        #[clap(long, default_value = "excluded_utxos_report.json")]
        excluded_report: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum UtxoCommands {
    /// Lists all UTXOs for the federation's known addresses
    List,
    /// Exports all UTXOs for the federation's known addresses to a file
    Export {
        /// The path to the file to export UTXOs to. If not provided, will print to stdout.
        #[clap(short, long)]
        output: Option<PathBuf>,
    },
    /// Compares the local UTXO set against an imported file
    Compare {
        /// The path to the file to import UTXOs from.
        #[clap(short, long)]
        input: PathBuf,
    },
}

fn validate_consensus_threshold(threshold: u8) -> Result<()> {
    if threshold == 0 || threshold > 100 {
        return Err(EmergencyToolError::InvalidConsensusThreshold { threshold });
    }
    Ok(())
}

fn validate_member_files(member_files: &[PathBuf]) -> Result<()> {
    if member_files.is_empty() {
        return Err(EmergencyToolError::NoMemberFiles);
    }

    if member_files.len() < 2 {
        return Err(EmergencyToolError::InsufficientMemberFiles {
            count: member_files.len(),
        });
    }

    for (i, file_path) in member_files.iter().enumerate() {
        let index = i + 1;
        
        if !file_path.exists() {
            return Err(EmergencyToolError::MemberFileNotFound {
                index,
                path: file_path.clone(),
            });
        }

        if !file_path.is_file() {
            return Err(EmergencyToolError::MemberFileNotRegular {
                index,
                path: file_path.clone(),
            });
        }

        // Check if file is readable by attempting to read metadata
        std::fs::metadata(file_path).map_err(|_| {
            EmergencyToolError::MemberFileNotAccessible {
                index,
                path: file_path.clone(),
            }
        })?;
    }

    Ok(())
}

fn validate_utxo_file(utxo_file: &PathBuf) -> Result<()> {
    if !utxo_file.exists() {
        return Err(EmergencyToolError::FileNotFound {
            path: utxo_file.clone(),
        });
    }

    if !utxo_file.is_file() {
        return Err(EmergencyToolError::NotRegularFile {
            path: utxo_file.clone(),
        });
    }

    // Check if file is readable
    std::fs::metadata(utxo_file).map_err(|_| EmergencyToolError::FileNotAccessible {
        path: utxo_file.clone(),
    })?;

    Ok(())
}

fn load_and_validate_utxos_from_file(file_path: &PathBuf) -> Result<Vec<Utxo>> {
    let file_content = std::fs::read_to_string(file_path).map_err(|_| {
        EmergencyToolError::UtxoFileReadFailed {
            path: file_path.clone(),
        }
    })?;

    if file_content.trim().is_empty() {
        return Err(EmergencyToolError::EmptyFile {
            path: file_path.clone(),
        });
    }

    let utxos: Vec<Utxo> = serde_json::from_str(&file_content).map_err(|_| {
        EmergencyToolError::UtxoFileJsonParseError {
            path: file_path.clone(),
        }
    })?;

    if utxos.is_empty() {
        return Err(EmergencyToolError::NoUtxosInFile {
            path: file_path.clone(),
        });
    }

    // Validate each UTXO
    for (i, utxo) in utxos.iter().enumerate() {
        if utxo.amount.to_sat() == 0 {
            return Err(EmergencyToolError::ZeroValueUtxoInFile {
                index: i + 1,
                path: file_path.clone(),
                txid: utxo.txid.to_string(),
                vout: utxo.vout,
            });
        }
    }

    Ok(utxos)
}

fn load_member_utxos(member_files: &[PathBuf]) -> Result<(Vec<Vec<Utxo>>, Vec<String>)> {
    let mut all_member_utxos = Vec::new();
    let mut member_labels = Vec::new();

    for (i, file_path) in member_files.iter().enumerate() {
        let member_utxos = load_and_validate_utxos_from_file(file_path).map_err(|_| {
            EmergencyToolError::MemberFileLoadFailed { index: i + 1 }
        })?;

        println!("  Loaded {} UTXOs from {}", member_utxos.len(), file_path.display());
        all_member_utxos.push(member_utxos);
        member_labels.push(format!("member_{}", i + 1));
    }

    Ok((all_member_utxos, member_labels))
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config: Config = confy::load("botanix-emergency-tool", None)
        .map_err(|_| EmergencyToolError::ConfigLoadFailed)?;

    let cli = Cli::parse();

    match cli.command {
        Commands::Utxo { command } => match command {
            UtxoCommands::List => {
                let btc_client = BtcClient::new(&config)
                    .map_err(|_| EmergencyToolError::BitcoinClientInitFailed)?;
                let utxos = btc_client.get_utxos().await
                    .map_err(|_| EmergencyToolError::UtxoRetrievalFailed)?;
                println!("{:#?}", utxos);
            }
            UtxoCommands::Export { output } => {
                let btc_client = BtcClient::new(&config)
                    .map_err(|_| EmergencyToolError::BitcoinClientInitFailed)?;
                let utxos = btc_client.get_utxos().await
                    .map_err(|_| EmergencyToolError::UtxoExportRetrievalFailed)?;
                
                if utxos.is_empty() {
                    println!("Warning: No UTXOs found to export");
                }
                
                let utxos_json = serde_json::to_string_pretty(&utxos)
                    .map_err(|_| EmergencyToolError::UtxoJsonSerializationFailed)?;
                
                if let Some(output_path) = output {
                    // Validate output directory exists
                    if let Some(parent) = output_path.parent() {
                        if !parent.exists() {
                            return Err(EmergencyToolError::OutputDirectoryNotFound {
                                path: parent.to_path_buf(),
                            });
                        }
                    }
                    
                    let mut file = File::create(&output_path).map_err(|_| {
                        EmergencyToolError::OutputFileCreationFailed {
                            path: output_path.clone(),
                        }
                    })?;
                    file.write_all(utxos_json.as_bytes()).map_err(|_| {
                        EmergencyToolError::UtxoFileWriteFailed {
                            path: output_path.clone(),
                        }
                    })?;
                    println!("Exported {} UTXOs to: {}", utxos.len(), output_path.display());
                } else {
                    println!("{}", utxos_json);
                }
            }
            UtxoCommands::Compare { input } => {
                // Validate input file
                if !input.exists() {
                    return Err(EmergencyToolError::FileNotFound { path: input });
                }

                let btc_client = BtcClient::new(&config)
                    .map_err(|_| EmergencyToolError::BitcoinClientInitFailed)?;
                let local_utxos = btc_client.get_utxos().await
                    .map_err(|_| EmergencyToolError::LocalUtxoRetrievalFailed)?;
                let local_utxos: HashSet<Utxo> = local_utxos.into_iter().collect();

                let mut file = File::open(&input).map_err(|_| {
                    EmergencyToolError::InputFileOpenFailed { path: input.clone() }
                })?;
                let mut contents = String::new();
                file.read_to_string(&mut contents).map_err(|_| {
                    EmergencyToolError::InputFileReadFailed { path: input.clone() }
                })?;

                if contents.trim().is_empty() {
                    return Err(EmergencyToolError::EmptyFile { path: input });
                }

                let remote_utxos: Vec<Utxo> = serde_json::from_str(&contents).map_err(|_| {
                    EmergencyToolError::InputFileJsonParseError { path: input }
                })?;
                
                if remote_utxos.is_empty() {
                    println!("Warning: Input file contains no UTXOs");
                }

                let remote_utxos: HashSet<Utxo> = remote_utxos.into_iter().collect();

                let in_local_not_remote: Vec<_> =
                    local_utxos.difference(&remote_utxos).collect();
                let in_remote_not_local: Vec<_> =
                    remote_utxos.difference(&local_utxos).collect();

                if in_local_not_remote.is_empty() && in_remote_not_local.is_empty() {
                    println!("UTXO sets are identical.");
                } else {
                    if !in_local_not_remote.is_empty() {
                        println!("UTXOs present locally but not in remote set:");
                        println!("{:#?}", in_local_not_remote);
                    }
                    if !in_remote_not_local.is_empty() {
                        println!("UTXOs present in remote set but not locally:");
                        println!("{:#?}", in_remote_not_local);
                    }
                }
            }
        },
        Commands::Sweep { destination, output, utxo_file, member_files, consensus_threshold, excluded_report } => {
            // Validate destination address is not empty
            if destination.trim().is_empty() {
                return Err(EmergencyToolError::EmptyDestination);
            }

            // Validate consensus threshold
            validate_consensus_threshold(consensus_threshold)?;

            // Validate that exactly one mode is specified
            match (&utxo_file, &member_files) {
                (Some(_), Some(_)) => {
                    return Err(EmergencyToolError::ConflictingModes);
                }
                (None, None) => {
                    return Err(EmergencyToolError::NoModeSpecified);
                }
                _ => {} // Exactly one is specified, which is correct
            }

            let btc_client = BtcClient::new(&config)
                .map_err(|_| EmergencyToolError::BitcoinClientInitFailed)?;
            
            // Check if this node is the coordinator before allowing PSBT construction
            if !btc_client.is_coordinator(&config)
                .map_err(|_| EmergencyToolError::CoordinatorStatusFailed)? {
                return Err(EmergencyToolError::NotCoordinator);
            }
            
            let final_utxos = if let Some(file_path) = utxo_file {
                // Manual curation mode: Use pre-curated UTXOs
                println!("Manual curation mode: Using pre-curated UTXOs");
                validate_utxo_file(&file_path)?;
                load_and_validate_utxos_from_file(&file_path)?
            } else if let Some(member_file_paths) = member_files {
                // Federation consensus mode: Use pre-exported UTXO files from members
                println!("Federation consensus mode: Processing {} member files", member_file_paths.len());
                
                validate_member_files(&member_file_paths)?;
                
                let (all_member_utxos, member_labels) = load_member_utxos(&member_file_paths)?;
                
                // Compute consensus for safe subset
                println!("Computing consensus for safe subset...");
                btc_client.compute_safe_subset(all_member_utxos, member_labels, consensus_threshold, Some(&excluded_report)).await?
            } else {
                unreachable!("Parameter validation should have caught this case");
            };
            
            // Construct PSBT
            println!("Constructing sweep PSBT...");
            btc_client.construct_sweep_psbt(final_utxos, &destination, &output).await?;
            println!("Emergency sweep PSBT constructed and saved to: {}", output.display());
        }
    }

    Ok(())
} 