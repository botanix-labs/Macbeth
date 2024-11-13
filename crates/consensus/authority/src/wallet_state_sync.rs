use std::time::Duration;

use bitcoin::{
    hashes::{sha256::Hash as Sha256Hash, FromSliceError},
    secp256k1::hashes::Hash,
};
use btcserverlib::extended_client::{BtcServerExtendedClient, GrpcClientError};
use client::{Empty, GetAllUtxosResponse, ResetAllUtxosRequest};
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

use crate::{
    compressor::{Compressor, Error as CompressorError, ProstMessageSerdelizer},
    utils::{generate_utxo_merkel_root, UtxoMerkelRootError},
    Storage,
};

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub(crate) enum WalletStateSyncError {
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
    #[error("Failed to generate utxo merkel root {0}")]
    UtxoMerkelRootError(#[from] UtxoMerkelRootError),
    #[error("UTXO set from peer is not in sync with the latest block, current utxo set merkel root: {0}, latest utxo set merkel root: {1}")]
    UtxoSetNotInSync(Sha256Hash, Sha256Hash),
    #[error("Failed to convert slide to sha256 hash {0}")]
    Sha256HashError(#[from] FromSliceError),
}

#[allow(dead_code)]
pub(crate) trait WalletStateSync {
    async fn sync_wallet_state(&self) -> Result<(), WalletStateSyncError>;
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct WalletStateSyncEngine<EF, BF, DB, ToFrostMan> {
    storage: Storage<EF, BF, DB>,
    btc_server: BtcServerExtendedClient,
    to_frost_manager: ToFrostMan,
    compressor: Compressor,
}

impl<EF, BF, DB, ToFrostMan> WalletStateSyncEngine<EF, BF, DB, ToFrostMan>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    ToFrostMan: ToFrostManager + Clone + 'static,
    DB: BlockReaderIdExt + Clone + 'static,
{
    pub(crate) fn new(
        storage: Storage<EF, BF, DB>,
        btc_server: BtcServerExtendedClient,
        to_frost_manager: ToFrostMan,
        compressor: Compressor,
    ) -> Self {
        Self { storage, btc_server, to_frost_manager, compressor }
    }
}

impl<EF, BF, DB, ToFrostMan> WalletStateSync for WalletStateSyncEngine<EF, BF, DB, ToFrostMan>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    ToFrostMan: ToFrostManager + Clone + 'static,
    DB: BlockReaderIdExt + Clone + 'static,
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

        // Since we are not in sync, we need to get the utxo set from a peer
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
                if let Some((_peer_id, peer_message)) = peer_message {
                    if let PeerMessageResponse::WalletState(wallet_state) = peer_message {
                        // TODO: refactor to update all wallet state

                        // process the utxo set
                        debug!(target: "consensus::authority::block_fetcher::sync_wallet_state", "Got utxo set from peer {:?}", wallet_state);
                        let data = wallet_state.data;
                        let decompressed = self.compressor.decompress(&data).await.map_err(|e| {
                            error!(target: "consensus::authority::block_fetcher::sync_wallet_state", "Failed to decompress utxo set data {:?}", e);
                            WalletStateSyncError::CompressorError(e)
                        })?;

                        let utxo_set = ProstMessageSerdelizer::<GetAllUtxosResponse>::deserialize(
                            decompressed,
                        )?;
                        // TODO peers will also need to communicate their tracked txs and pending
                        // pegouts
                        // let merkel_root = generate_utxo_merkel_root(&utxo_set.utxos)?;
                        // if merkel_root != latest_utxo_commitment {
                        //     return Err(WalletStateSyncError::UtxoSetNotInSync(
                        //         merkel_root,
                        //         latest_utxo_commitment,
                        //     ));
                        // }

                        // Report to btc server to sync utxo set
                        btc_server
                            .reset_all_utxos(ResetAllUtxosRequest { utxos: utxo_set.utxos })
                            .await?;
                    }
                } else {
                    // TODO better error variant
                    return Err(WalletStateSyncError::PeerUtxoSetTimeout);
                }
            }
            Err(e) => {
                warn!(target: "consensus::authority::block_fetcher::sync_utxo_set", ?e, "Failed to get get utxo set rom a peer within 60 secs");
                return Err(WalletStateSyncError::PeerUtxoSetTimeout);
            }
        }

        Ok(())
    }
}
