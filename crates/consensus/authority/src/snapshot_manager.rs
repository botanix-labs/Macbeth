use crate::{comet_bft::abci::ABCIDriverMessage, Storage};
use bitcoin::hashes::{sha256::Hash as Sha256Hash, FromSliceError};
use btcserverlib::extended_client::GrpcClientError;
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_data_parser::{DataParser, Error as DataParserError};
use reth_evm::execute::BlockExecutorProvider;
use reth_network::frost::manager::FrostCommand;
use reth_primitives::extra_data_header::ExtraDataHeaderDeserializeError;
use reth_provider::{BlockReaderIdExt, ProviderError};
use tokio::sync::mpsc::error::SendError;
use tracing::{debug, error, trace, warn};

pub const DEFAULT_SNAPSHOT_INTERVAL: u64 = 1000;
pub const DEFAULT_SNAPSHOT_KEEP_RECENT: u64 = 2;

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum SnapshotManagerError {
    #[error("db provider error: {0}")]
    LatestBlockError(#[from] ProviderError),
    #[error("deserilaize extra data header : {0}")]
    DeserializeExtraDataHeaderError(#[from] ExtraDataHeaderDeserializeError),
    #[error("btc server client error: {0}")]
    BtcServerClientError(#[from] GrpcClientError),
    #[error("frost manager send error: {0}")]
    FrostManagerSendError(#[from] SendError<FrostCommand>),
    #[error("peer never responded with utxo set, timer elapsed")]
    PeerUtxoSetTimeout,
    #[error("Failed to receive a frost message from a peer {0}")]
    FrostRecv(tokio::sync::oneshot::error::RecvError),
    #[error("Data parser error: {0}")]
    DataParser(#[from] DataParserError),
    #[error("UTXO set from peer is not in sync with the latest block, current utxo set merkel root: {0}, latest utxo set merkel root: {1}")]
    UtxoSetNotInSync(Sha256Hash, Sha256Hash),
    #[error("Failed to convert slide to sha256 hash {0}")]
    Sha256HashError(#[from] FromSliceError),
}

/// Snapshot manager monitoring trait
#[allow(dead_code)]
pub trait SnapshotCoordinator {
    fn run(&mut self)
        -> impl std::future::Future<Output = Result<(), SnapshotManagerError>> + Send;
}

/// Snapshot manager is responsible for persisting snapshot chunks to disk
#[allow(dead_code)]
pub struct SnapshotManager<EF, BF, DB> {
    storage: Storage<EF, BF, DB>,
    compressor: DataParser,
    snapshot_manager_tx: tokio::sync::mpsc::Receiver<ABCIDriverMessage>,
    snapshot_interval: u64,
    snapshot_keep_recent: u64,
}

impl<EF, BF, DB> SnapshotManager<EF, BF, DB>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    DB: BlockReaderIdExt + Clone + 'static,
{
    pub(crate) fn new(
        storage: Storage<EF, BF, DB>,
        compressor: DataParser,
        snapshot_manager_tx: tokio::sync::mpsc::Receiver<ABCIDriverMessage>,
    ) -> Self {
        Self {
            storage,
            compressor,
            snapshot_manager_tx,
            snapshot_interval: DEFAULT_SNAPSHOT_INTERVAL,
            snapshot_keep_recent: DEFAULT_SNAPSHOT_KEEP_RECENT,
        }
    }
}

impl<EF, BF, DB> SnapshotCoordinator for SnapshotManager<EF, BF, DB>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    DB: BlockReaderIdExt + Clone + 'static,
{
    // Note: this function should not be called unless we are fully synced
    async fn run(&mut self) -> Result<(), SnapshotManagerError> {
        trace!(target: "consensus::authority::snapshot_manager::run", "started");
        let client = self.storage.client.clone();

        //client.block_body_indices(num)

        let latest_header = client.latest_header()?.expect("should get latest block");
        // let latest_merkle_root = latest_header.get_utxo_set_merkle_root()?;

        if latest_header.number == 0 {
            debug!(target: "consensus::authority::StateSync::sync_utxo_set", "genesis block");
            return Ok(());
        }

        while let Some(abci_driver_message) = self.snapshot_manager_tx.recv().await {
            debug!(target: "consensus::authority::snapshot_manager::run", "received abci driver message {:?}", abci_driver_message);

            match abci_driver_message {
                ABCIDriverMessage::CommitBlock((sealed_block_with_peg, cbft_hash, tx)) => {
                    tx.send(()).expect("to send");

                    let sealed_block = sealed_block_with_peg.block();

                    let serialized_compressed_sealed_block = self
                        .compressor
                        .encode(sealed_block)
                        .await
                        .map_err(|e| {
                            error!(target:"consensus::authority::snapshot_manager", "Failed to serialize and compress sealed block {:?}", e);
                            SnapshotManagerError::DataParser(e)
                        })?;

                    // TODO: save into db
                }
                ABCIDriverMessage::Exit => {
                    debug!(target: "consensus::authority::snapshot_manager::run", "exiting");
                    return Ok(());
                }
            }
        }

        Ok(())
    }
}
