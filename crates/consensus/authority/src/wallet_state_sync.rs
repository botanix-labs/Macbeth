//! Wallet state sync module
use bitcoin::hashes::{sha256::Hash as Sha256Hash, FromSliceError};
use btcserverlib::extended_client::{BtcServerExtendedApi, GrpcClientError};
use client::{GetPendingPegoutsResponse, ResetWalletStateRequest};
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_data_parser::{DataParser, Error as CompressorError, SerializationType};
use reth_evm::execute::BlockExecutorProvider;
use reth_network::frost::{
    manager::{FrostCommand, ToFrostManager},
    PeerMessageResponse,
};
use reth_primitives::extra_data_header::ExtraDataHeaderDeserializeError;
use reth_provider::{BlockReaderIdExt, ProviderError};
use std::time::Duration;
use tokio::sync::mpsc::error::SendError;
use tracing::{debug, error, info, trace, warn};

use crate::{
    prost_parser::{ProstError, ProstMessageSerdelizer},
    utils::UtxoMerkelRootError,
    Storage,
};

#[derive(Debug, thiserror::Error)]
/// Wallet state synchronization errors
pub enum WalletStateSyncError {
    #[error("db provider error: {0}")]
    /// Latest block error
    LatestBlockError(#[from] ProviderError),
    #[error("deserilaize extra data header : {0}")]
    /// Extra data header deserialize error
    DeserializeExtraDataHeaderError(#[from] ExtraDataHeaderDeserializeError),
    #[error("btc server client error: {0}")]
    /// Btc server client error
    BtcServerClientError(#[from] GrpcClientError),
    #[error("frost manager send error: {0}")]
    /// Frost manager send error
    FrostManagerSendError(#[from] SendError<FrostCommand>),
    #[error("peer never responded with wallet state, timer elapsed")]
    /// Peer wallet state timeout
    PeerWalletStateTimeout,
    #[error("Failed to receive a frost message from a peer {0}")]
    /// Frost recv error
    FrostRecv(tokio::sync::oneshot::error::RecvError),
    #[error("Failed to decompress wallet state data {0}")]
    /// Compressor error
    CompressorError(#[from] CompressorError),
    #[error("Failed to deserialize wallet state data {0}")]
    /// Prost error
    ProstError(#[from] ProstError),
    #[error("Failed to generate utxo merkel root {0}")]
    /// Utxo merkel root error
    UtxoMerkelRootError(#[from] UtxoMerkelRootError),
    #[error("UTXO set from peer is not in sync with the latest block, current utxo set merkel root: {0}, latest utxo set merkel root: {1}")]
    /// Utxo set not in sync
    UtxoSetNotInSync(Sha256Hash, Sha256Hash),
    #[error("Failed to convert slide to sha256 hash {0}")]
    /// Sha256 hash error
    Sha256HashError(#[from] FromSliceError),
}

/// Trait for synchronizing wallet state
#[allow(async_fn_in_trait)]
pub trait WalletStateSync {
    /// Synchronizes the wallet state
    async fn sync_wallet_state(&self) -> Result<(), WalletStateSyncError>;
}

#[derive(Clone)]
/// Engine for synchronizing wallet state
pub struct WalletStateSyncEngine<EF, BF, DB, ToFrostMan, BtcServerClient> {
    storage: Storage<EF, BF, DB>,
    btc_server: BtcServerClient,
    to_frost_manager: ToFrostMan,
    data_parser: DataParser,
}

impl<EF, BF, DB, ToFrostMan, BtcServerClient>
    WalletStateSyncEngine<EF, BF, DB, ToFrostMan, BtcServerClient>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    ToFrostMan: ToFrostManager + Clone + 'static,
    DB: BlockReaderIdExt + Clone + 'static,
    BtcServerClient: BtcServerExtendedApi + Clone,
{
    pub(crate) fn new(
        storage: Storage<EF, BF, DB>,
        btc_server: BtcServerClient,
        to_frost_manager: ToFrostMan,
    ) -> Self {
        let data_parser =
            DataParser::default().with_serialization_type(SerializationType::Postcard);
        Self { storage, btc_server, to_frost_manager, data_parser }
    }
}

impl<EF, BF, DB, ToFrostMan, BtcServerClient> WalletStateSync
    for WalletStateSyncEngine<EF, BF, DB, ToFrostMan, BtcServerClient>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    ToFrostMan: ToFrostManager + Clone + 'static,
    DB: BlockReaderIdExt + Clone + 'static,
    BtcServerClient: BtcServerExtendedApi + Clone,
{
    // Note: this function should not be called unless we are fully synced
    async fn sync_wallet_state(&self) -> Result<(), WalletStateSyncError> {
        trace!(target: "consensus::authority::UTXOSync::sync_utxo_set", "syncing utxo set");
        let client = self.storage.client.clone();
        let mut btc_server = self.btc_server.clone();

        let latest_header = client.latest_header()?.expect("should get latest block");
        if latest_header.number == 0 {
            debug!(target: "consensus::authority::UTXOSync::sync_utxo_set", "genesis block, no utxo set to sync");
            return Ok(());
        }

        let (peer_messages_tx, peer_messages_rx) = tokio::sync::oneshot::channel();
        self.to_frost_manager
            .send_command(FrostCommand::GetPeerMessagesStream(peer_messages_tx))?;
        let mut peer_messages_rx = peer_messages_rx.await.expect("peer messages rx to be open");

        // Request the wallet state from a peer
        // TODO should sample many wallet states from N peers
        self.to_frost_manager.send_command(FrostCommand::GetWalletStateFromPeer)?;
        // try getting the wallet state from the peers we requested it from
        match tokio::time::timeout(Duration::from_secs(60), peer_messages_rx.recv()).await {
            Ok(peer_message) => {
                if let Some(peer_message_context) = peer_message {
                    info!(target: "consensus::authority::sync_wallet_state", "Received wallet state from peer {:?}", peer_message_context.peer_id);

                    // Note: we ignore empty messages bc they are peer requests for wallet state
                    // which are handled by the frost task or are malicious/faulty requests
                    // that would cause the btc-server to wipe its state
                    if let PeerMessageResponse::WalletState(wallet_state) =
                        peer_message_context.message
                    {
                        // process the utxos
                        debug!(target: "consensus::authority::sync_wallet_state", "Received wallet state from peer {:?}", wallet_state);

                        // process the pending pegouts
                        let pending_pegouts_compressed = wallet_state.pending_pegouts;
                        let pending_pegouts = {
                            if pending_pegouts_compressed.is_empty() {
                                warn!(target: "consensus::authority::sync_wallet_state", "Peer sent empty pending pegouts");
                                return Ok(());
                            } else {
                                let pending_pegouts_decompressed = self.data_parser.decompress(&pending_pegouts_compressed).await.map_err(|e| {
                                    error!(target: "consensus::authority::sync_wallet_state", "Failed to decompress pending pegouts {:?}", e);
                                    WalletStateSyncError::CompressorError(e)
                                })?;
                                ProstMessageSerdelizer::<GetPendingPegoutsResponse>::deserialize(
                                    pending_pegouts_decompressed,
                                )?
                                .pending_pegouts
                            }
                        };

                        // Report to btc server to reset the wallet state
                        btc_server
                            .reset_wallet_state(ResetWalletStateRequest {
                                pending_pegouts: pending_pegouts.clone(),
                            })
                            .await?;
                    }
                } else {
                    return Err(WalletStateSyncError::PeerWalletStateTimeout);
                }
            }
            Err(e) => {
                warn!(target: "consensus::authority::sync_wallet_state", ?e, "Failed to get get wallet state from a peer within 60 secs");
                return Err(WalletStateSyncError::PeerWalletStateTimeout);
            }
        }

        Ok(())
    }
}
