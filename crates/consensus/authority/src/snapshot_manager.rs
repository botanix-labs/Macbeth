use crate::{
    comet_bft::abci::ABCIDriverMessage,
    compressor::{Compressor, Error as CompressorError, ProstMessageSerdelizer},
    Storage,
};
use bitcoin::hashes::{sha256::Hash as Sha256Hash, FromSliceError};
use btcserverlib::extended_client::{BtcServerExtendedClient, GrpcClientError};
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_evm::execute::BlockExecutorProvider;
use reth_network::frost::{
    manager::{FrostCommand, ToFrostManager},
    PeerMessageResponse,
};
use reth_primitives::extra_data_header::ExtraDataHeaderDeserializeError;
use reth_provider::{BlockReaderIdExt, ProviderError};
use tokio::sync::mpsc::error::SendError;
use tracing::{debug, error, trace, warn};

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
    #[error("Failed to decompress utxo set data {0}")]
    CompressorError(#[from] CompressorError),
    #[error("UTXO set from peer is not in sync with the latest block, current utxo set merkel root: {0}, latest utxo set merkel root: {1}")]
    UtxoSetNotInSync(Sha256Hash, Sha256Hash),
    #[error("Failed to convert slide to sha256 hash {0}")]
    Sha256HashError(#[from] FromSliceError),
}

/// Snapshot manager monitoring trait
#[allow(dead_code)]
pub trait SnapshotCoordinator {
    async fn run(&mut self) -> Result<(), SnapshotManagerError>;
}

/// Snapshot manager is responsible for persisting snapshot chunks to disk
#[allow(dead_code)]
#[derive(Debug)]
pub struct SnapshotManager<EF, BF, DB> {
    storage: Storage<EF, BF, DB>,
    compressor: Compressor,
    snapshot_manager_tx: tokio::sync::mpsc::Receiver<ABCIDriverMessage>,
}

impl<EF, BF, DB> SnapshotManager<EF, BF, DB>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    DB: BlockReaderIdExt + Clone + 'static,
{
    pub(crate) fn new(
        storage: Storage<EF, BF, DB>,
        compressor: Compressor,
        snapshot_manager_tx: tokio::sync::mpsc::Receiver<ABCIDriverMessage>,
    ) -> Self {
        Self { storage, compressor, snapshot_manager_tx }
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

        let latest_header = client.latest_header()?.expect("should get latest block");
        // let latest_merkle_root = latest_header.get_utxo_set_merkle_root()?;

        if latest_header.number == 0 {
            debug!(target: "consensus::authority::StateSync::sync_utxo_set", "genesis block");
            return Ok(());
        }

        while let Some(abci_driver_message) = self.snapshot_manager_tx.recv().await {
            debug!(target: "consensus::authority::snapshot_manager::run", "received abci driver message {:?}", abci_driver_message);

            match abci_driver_message {
                ABCIDriverMessage::CommitBlock((_sealed_block_with_peg, _cbft_hash, tx)) => {
                    tx.send(()).expect("to send");
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
