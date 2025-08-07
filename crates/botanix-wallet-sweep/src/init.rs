// use crate::dump::UtxoDumpsReader;
use crate::dump::UtxoDumpsReader;
use bitcoin::{Address, Network};
use bitcoincore_rpc::Client;
use botanix_data_parser::DataParser;
use botanix_storage::{models::WalletSweepSession, tables::Compress};
use btc_server_client::{
    AcceptWalletSweepSessionRequest, BtcServerClient, BtcServerExtendedApi, BtcServerExtendedClient,
};
use std::{fmt::Debug, str::FromStr, time::SystemTime};
use tracing::{info, warn};

pub trait DestinationConfig: Debug {
    fn network(&self) -> eyre::Result<bitcoin::Network>;
    fn address(&self) -> eyre::Result<bitcoin::Address>;

    fn fee_rate(&self) -> eyre::Result<bitcoin::FeeRate>;
}

pub trait UtxoConfig: Debug {}

pub async fn init_wallet_sweep(
    btc_server_client: &mut BtcServerExtendedClient,
    utxo_dump_parser: DataParser,
    destination: impl DestinationConfig,
    utxo_dumps_reader: &UtxoDumpsReader,
) -> eyre::Result<()> {
    info!("Starting emergency sweep initiation");
    // info!(
    //     "Destination: {}, Fee rate: {} sat/vB, Consensus threshold: {}%",
    //     destination, fee_rate, consensus_threshold
    // );
    // info!(target: "reth::cli", "Federation config: {}, Coordinator key: {}",
    //           federation_config.display(), coordinator_key.display());
    // info!(target: "reth::cli", "Timeout: {}s, Chunk size: {}", timeout, chunk_size);
    //
    // if let Some(jwt_path) = jwt_secret {
    //     info!(target: "reth::cli", "JWT secret: {}", jwt_path.display());
    // }

    // TODO: Read dumps
    // TODO: Validate that we can ensure threshold
    // TODO: Intersect UTXOs
    // TODO: Create PSBT

    // TODO: We need to create request, then create session from request
    let created_at = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    let session = WalletSweepSession {
        psbt_bytes: Default::default(),
        bitcoin_network: Network::Bitcoin,
        bitcoin_destination_address: Address::from_str("bc1qexampleaddress1234567890abcdefg")?,
        created_at,
    };

    let request = AcceptWalletSweepSessionRequest { request: session.compress() };

    btc_server_client.accept_wallet_sweep_session(request).await?;

    // TODO: Write request to file and print some information

    Ok(())
}
