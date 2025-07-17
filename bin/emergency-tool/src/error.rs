use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum EmergencyToolError {
    #[error("Bitcoin RPC error: {0}")]
    BitcoinRpc(#[from] bitcoincore_rpc::Error),

    #[error("Bitcoin library error: {0}")]
    Bitcoin(#[from] bitcoin::address::ParseError),

    #[error("JSON serialization error: {0}")]
    JsonSerialization(#[from] serde_json::Error),

    #[error("File I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(#[from] confy::ConfyError),

    #[error("PSBT error: {0}")]
    Psbt(#[from] bitcoin::psbt::Error),

    // Configuration validation errors
    #[error("Bitcoin RPC URL cannot be empty")]
    EmptyBitcoinRpcUrl,

    #[error("Bitcoin RPC username cannot be empty")]
    EmptyBitcoinRpcUser,

    #[error("Bitcoin RPC password cannot be empty")]
    EmptyBitcoinRpcPass,

    #[error("Federation config file not found: {path}. Please ensure the file exists and is readable.")]
    FederationConfigNotFound { path: PathBuf },

    // Coordinator errors
    #[error("PSBT construction is restricted to the designated coordinator. Please run this command on the coordinator node.")]
    NotCoordinator,

    // Parameter validation errors
    #[error("Destination address cannot be empty")]
    EmptyDestination,

    #[error("Invalid destination address format: '{address}'")]
    InvalidDestinationAddress { address: String },

    #[error("Consensus threshold must be between 1 and 100, got: {threshold}")]
    InvalidConsensusThreshold { threshold: u8 },

    #[error("Cannot specify both --utxo-file and --member-files. Use --utxo-file for manual curation or --member-files for federation consensus.")]
    ConflictingModes,

    #[error("Must specify either --utxo-file for manual curation or --member-files for federation consensus")]
    NoModeSpecified,

    // File validation errors
    #[error("File does not exist: {path}")]
    FileNotFound { path: PathBuf },

    #[error("Path is not a regular file: {path}")]
    NotRegularFile { path: PathBuf },

    #[error("Output directory does not exist: {path}. Please create the directory first.")]
    OutputDirectoryNotFound { path: PathBuf },

    #[error("File is empty: {path}")]
    EmptyFile { path: PathBuf },

    #[error("Cannot access file: {path}")]
    FileNotAccessible { path: PathBuf },

    // Member files validation errors
    #[error("No member files provided. At least one member file is required for consensus mode.")]
    NoMemberFiles,

    #[error("Federation consensus requires at least 2 member files, got: {count}. For single-member operations, use --utxo-file instead.")]
    InsufficientMemberFiles { count: usize },

    #[error("Member file {index} does not exist: {path}")]
    MemberFileNotFound { index: usize, path: PathBuf },

    #[error("Member file {index} is not a regular file: {path}")]
    MemberFileNotRegular { index: usize, path: PathBuf },

    #[error("Cannot access member file {index}: {path}")]
    MemberFileNotAccessible { index: usize, path: PathBuf },

    #[error("Failed to load UTXOs from member file {index}")]
    MemberFileLoadFailed { index: usize },

    // UTXO validation errors
    #[error("No UTXOs provided for PSBT construction")]
    NoUtxosProvided,

    #[error("UTXO file contains no UTXOs: {path}")]
    NoUtxosInFile { path: PathBuf },

    #[error("UTXO {index} in file {path} has zero value: {txid}:{vout}")]
    ZeroValueUtxoInFile {
        index: usize,
        path: PathBuf,
        txid: String,
        vout: u32,
    },

    #[error("Invalid UTXO {txid}:{vout} has zero value")]
    ZeroValueUtxo { txid: String, vout: u32 },

    // Consensus computation errors
    #[error("No member UTXO sets provided for consensus computation")]
    NoMemberUtxoSets,

    #[error("Member labels count ({labels_count}) does not match UTXO sets count ({sets_count})")]
    MemberLabelsMismatch {
        labels_count: usize,
        sets_count: usize,
    },

    #[error("Required votes ({required}) cannot exceed total members ({total})")]
    RequiredVotesExceedsMembers { required: usize, total: usize },

    #[error("No valid UTXOs discovered from any federation member")]
    NoValidUtxosDiscovered,

    #[error("No UTXOs achieved consensus threshold. {excluded_count} UTXOs were excluded. Consider lowering the consensus threshold or checking member connectivity.")]
    NoConsensusAchieved { excluded_count: usize },

    // Transaction construction errors
    #[error("Insufficient funds to cover transaction fees. Total input value: {total_input}, Estimated fee: {estimated_fee}. Need at least {shortage} more satoshis.")]
    InsufficientFunds {
        total_input: u64,
        estimated_fee: u64,
        shortage: u64,
    },

    #[error("Output value {output_value} is below dust threshold {dust_threshold}")]
    OutputBelowDustThreshold {
        output_value: u64,
        dust_threshold: u64,
    },

    #[error("Failed to create PSBT from transaction")]
    PsbtCreationFailed,

    #[error("Failed to write PSBT to {path}")]
    PsbtWriteFailed { path: PathBuf },

    #[error("Failed to serialize excluded UTXO report")]
    ExcludedReportSerializationFailed,

    #[error("Failed to write excluded UTXO report to {path}")]
    ExcludedReportWriteFailed { path: PathBuf },

    // Bitcoin RPC specific errors
    #[error("Failed to create Bitcoin RPC client")]
    BitcoinRpcClientCreationFailed,

    #[error("Failed to retrieve UTXOs from Bitcoin RPC")]
    UtxoRetrievalFailed,

    #[error("Failed to retrieve UTXOs for export")]
    UtxoExportRetrievalFailed,

    #[error("Failed to retrieve local UTXOs")]
    LocalUtxoRetrievalFailed,

    // JSON parsing errors
    #[error("Failed to parse UTXO file as JSON: {path}")]
    UtxoFileJsonParseError { path: PathBuf },

    #[error("Failed to parse input file as JSON: {path}")]
    InputFileJsonParseError { path: PathBuf },

    #[error("Failed to serialize UTXOs to JSON")]
    UtxoJsonSerializationFailed,

    // File operation errors
    #[error("Failed to read UTXO file: {path}")]
    UtxoFileReadFailed { path: PathBuf },

    #[error("Failed to read input file: {path}")]
    InputFileReadFailed { path: PathBuf },

    #[error("Failed to create output file: {path}")]
    OutputFileCreationFailed { path: PathBuf },

    #[error("Failed to write UTXOs to file: {path}")]
    UtxoFileWriteFailed { path: PathBuf },

    #[error("Failed to open input file: {path}")]
    InputFileOpenFailed { path: PathBuf },

    // Initialization errors
    #[error("Failed to initialize Bitcoin client")]
    BitcoinClientInitFailed,

    #[error("Failed to load configuration. Please check your config file format.")]
    ConfigLoadFailed,

    #[error("Failed to determine coordinator status")]
    CoordinatorStatusFailed,
}

pub type Result<T> = std::result::Result<T, EmergencyToolError>; 