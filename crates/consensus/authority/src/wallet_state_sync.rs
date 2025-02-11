//! Wallet state sync module
use std::{sync::Arc, time::Duration};

use bitcoin::hashes::FromSliceError;
use btcserverlib::extended_client::{BtcServerExtendedApi, GrpcClientError};
use client::{
    GetAllUtxosResponse, GetPendingPegoutsResponse, GetTrackedTxsResponse, ResetWalletStateRequest,
};
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_data_parser::{DataParser, Error as DataParserError};
use reth_evm::execute::BlockExecutorProvider;
use reth_network::frost::{
    manager::{FrostCommand, ToFrostManager},
    PeerMessageResponse,
};
use reth_primitives::extra_data_header::ExtraDataHeaderDeserializeError;
use reth_provider::{BlockReaderIdExt, ProviderError};
use tokio::sync::mpsc::error::SendError;
use tracing::{debug, error, info, trace, warn};

use crate::{
    metrics::AuthorityMetrics,
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
    #[error("Failed to decompress utxo set data {0}")]
    /// Prost error
    Prost(#[from] ProstError),
    /// Data parser error
    #[error("Data Parser Error: {0}")]
    DataParser(#[from] DataParserError),
    #[error("Failed to generate utxo merkel root {0}")]
    /// Utxo merkel root error
    UtxoMerkelRootError(#[from] UtxoMerkelRootError),
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
    compressor: DataParser,
    metrics: Arc<AuthorityMetrics>,
}

impl<EF, BF, DB, ToFrostMan, BtcServerClient>
    WalletStateSyncEngine<EF, BF, DB, ToFrostMan, BtcServerClient>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    ToFrostMan: ToFrostManager + Clone + 'static,
    DB: BlockReaderIdExt + Clone + 'static,
    BtcServerClient: BtcServerExtendedApi + Clone + 'static,
{
    pub(crate) fn new(
        storage: Storage<EF, BF, DB>,
        btc_server: BtcServerClient,
        to_frost_manager: ToFrostMan,
        compressor: DataParser,
        metrics: Arc<AuthorityMetrics>,
    ) -> Self {
        Self { storage, btc_server, to_frost_manager, compressor, metrics }
    }
}

impl<EF, BF, DB, ToFrostMan, BtcServerClient> WalletStateSync
    for WalletStateSyncEngine<EF, BF, DB, ToFrostMan, BtcServerClient>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    ToFrostMan: ToFrostManager + Clone + 'static,
    DB: BlockReaderIdExt + Clone + 'static,
    BtcServerClient: BtcServerExtendedApi + Clone + 'static,
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
        // try getting the wallet state from the random peer we pinged
        match tokio::time::timeout(Duration::from_secs(60), peer_messages_rx.recv()).await {
            Ok(peer_message) => {
                if let Some(message_context) = peer_message {
                    let peer_message = message_context.message;
                    let peer_id = message_context.peer_id;
                    info!(target: "consensus::authority::sync_wallet_state", "Received wallet state from peer {:?}", peer_id);

                    // Note: we ignore empty messages bc they are peer requests for wallet state
                    // which are handled by the frost task or are malicious/faulty requests
                    // that would cause the btc-server to wipe its state
                    if let PeerMessageResponse::WalletState(wallet_state) = peer_message {
                        // process the utxos
                        debug!(target: "consensus::authority::sync_wallet_state", "Received wallet state from peer {:?}", wallet_state);
                        let utxos_compressed = wallet_state.utxos;
                        let utxos = {
                            if utxos_compressed.is_empty() {
                                warn!(target: "consensus::authority::sync_wallet_state", "Peer sent empty utxos");
                                return Ok(());
                            } else {
                                let utxos_decompressed = self.compressor.decompress(&utxos_compressed).await.map_err(|e| {
                                    error!(target: "consensus::authority::sync_wallet_state", "Failed to decompress utxos {:?}", e);
                                    WalletStateSyncError::DataParser(e)
                                })?;
                                ProstMessageSerdelizer::<GetAllUtxosResponse>::deserialize(
                                    utxos_decompressed,
                                )?
                                .utxos
                            }
                        };

                        // process the tracked txs
                        let tracked_txs_compressed = wallet_state.tracked_txs;
                        let tracked_txs = {
                            if tracked_txs_compressed.is_empty() {
                                warn!(target: "consensus::authority::sync_wallet_state", "Peer sent empty tracked txs");
                                return Ok(());
                            } else {
                                let tracked_txs_decompressed = self.compressor.decompress(&tracked_txs_compressed).await.map_err(|e| {
                                    error!(target: "consensus::authority::sync_wallet_state", "Failed to decompress tracked txs {:?}", e);
                                    WalletStateSyncError::DataParser(e)
                                })?;
                                ProstMessageSerdelizer::<GetTrackedTxsResponse>::deserialize(
                                    tracked_txs_decompressed,
                                )?
                                .tracked_txs
                            }
                        };

                        // process the pending pegouts
                        let pending_pegouts_compressed = wallet_state.pending_pegouts;
                        let pending_pegouts = {
                            if pending_pegouts_compressed.is_empty() {
                                warn!(target: "consensus::authority::sync_wallet_state", "Peer sent empty pending pegouts");
                                return Ok(());
                            } else {
                                let pending_pegouts_decompressed = self.compressor.decompress(&pending_pegouts_compressed).await.map_err(|e| {
                                    error!(target: "consensus::authority::sync_wallet_state", "Failed to decompress pending pegouts {:?}", e);
                                    WalletStateSyncError::DataParser(e)
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
                                utxos: utxos.clone(),
                                tracked_txs: tracked_txs.clone(),
                                pending_pegouts: pending_pegouts.clone(),
                            })
                            .await?;
                        self.metrics.reset_wallet_states.increment(1);
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
