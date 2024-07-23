use std::collections::HashMap;

use crate::{
    compressor::{Compressor, Error as CompressorError, ProstMessageSerdelizer},
    utils::{check_all_peers_initially_connected, is_active_sync_in_progress},
    Storage,
};
use btcserverlib::extended_client::{BtcServerExtendedClient, GrpcClientError};
use reth_network::{
    frost::{
        manager::{FrostCommand, PeerData, ToFrostManager},
        FrostPeerCommand, PeerMessageResponse,
    },
    NetworkHandle,
};
use reth_rpc_types::PeerId;
use tokio::sync::{mpsc::UnboundedReceiver, oneshot::error::RecvError};
use tracing::{error, info, warn};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Failed to receive a frost message from a peer {0}")]
    FrostRecv(RecvError),
    #[error("Received a grpc client error {0}")]
    Grpc(GrpcClientError),
    #[error("compressor error {0}")]
    Compressor(CompressorError),
}

/// The utxo sync task is responsible for running in th the background and receiving messages
/// from counterpeers who want to sync their utxo set with us. Upon receiving a request, the
/// task shall reply with the serialized utxo set back to the caller in an asynchronous manner
/// over the gossiping protocol.
pub struct UtxoSyncTask<ToFrostMan> {
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    /// Frost network Handler
    pub(crate) frost_handle: ToFrostMan,
    /// Shared authority storage
    pub(crate) storage: Storage,
    /// Btc Server client
    pub(crate) btc_server: BtcServerExtendedClient,
    /// compressor
    pub(crate) compressor: Compressor,
}

impl<ToFrostMan> UtxoSyncTask<ToFrostMan>
where
    ToFrostMan: ToFrostManager + Clone + Send + 'static,
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        network_handle: NetworkHandle,
        frost_handle: ToFrostMan,
        storage: Storage,
        btc_server: BtcServerExtendedClient,
    ) -> Self {
        Self { network_handle, frost_handle, storage, btc_server, compressor: Compressor::new() }
    }

    async fn get_serialized_compressed_utxo_set(&mut self) -> Result<Vec<u8>, Error> {
        let prost_utxos = self.btc_server.get_all_utxos(client::Empty {}).await.map_err(|e| {
            error!(target: "consensus::authority::utxo_syncer::get_utxo_set", "Got grpc error {:?}", e);
            Error::Grpc(e)
        })?;

        // serialize the prost message
        let prost_message_wrapper = ProstMessageSerdelizer(prost_utxos);
        let prost_serialized = prost_message_wrapper.serialize().map_err(|e| {
            error!(target: "consensus::authority::utxo_syncer::get_utxo_set", "Got compressor error {:?}", e);
            Error::Compressor(e)
        })?;

        // now compress the prost message
        let prost_serialized_compressed = self.compressor.compress(&prost_serialized).await.map_err(|e| {
            error!(target: "consensus::authority::utxo_syncer::get_utxo_set", "Got compressor error {:?}", e);
            Error::Compressor(e)
        })?;
        Ok(prost_serialized_compressed)
    }

    async fn get_peer_messages_rx(
        &self,
    ) -> Result<UnboundedReceiver<(PeerId, PeerMessageResponse)>, Error> {
        // get a proper event receiver
        let (peer_messages_tx, peer_messages_rx) = tokio::sync::oneshot::channel();
        if let Err(e) =
            self.frost_handle.send_command(FrostCommand::GetPeerMessagesStream(peer_messages_tx))
        {
            error!(target: "consensus::authority::utxo_syncer::get_peer_messages_rx", "Failed to send GetPeerMessagesStream frost command {}", e);
        }
        let peer_messages_rx = peer_messages_rx.await.map_err(|e| {
            error!(target: "consensus::authority::utxo_syncer::get_peer_messages_rx", "Error getting receiver handle = {:?}", e);
            Error::FrostRecv(e)
        })?;
        Ok(peer_messages_rx)
    }

    async fn get_all_connected_authority_peers(&self) -> Result<HashMap<PeerId, PeerData>, Error> {
        let (connected_peers_tx, connected_peers_rx) = tokio::sync::oneshot::channel();
        if let Err(e) =
            self.frost_handle.send_command(FrostCommand::GetAllConnectedPeers(connected_peers_tx))
        {
            error!(target: "consensus::authority::utxo_syncer::get_all_connected_authority_peers", "Failed to send GetAllConnectedPeers frost command {:?}", e);
        }
        let connected_peers = connected_peers_rx.await.map_err(|e| {
            error!(target: "consensus::authority::utxo_syncer::get_all_connected_authority_peers", "Error getting receiver handle = {:?}", e);
            Error::FrostRecv(e)
        })?;
        Ok(connected_peers)
    }

    pub async fn start_task(&mut self) {
        info!(target: "consensus::authority::utxo_syncer::start_task", "Starting Utxo Sync Task");

        // await all peers to be connected
        loop {
            if check_all_peers_initially_connected(&self.frost_handle).await {
                break;
            }
            // short sleep
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }

        // ensure the node is not syncing
        loop {
            if !is_active_sync_in_progress(&self.network_handle) {
                break;
            }
            // short sleep
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }

        // get all connected authority peers
        let connected_peers = match self.get_all_connected_authority_peers().await {
            Ok(connected_peers) => connected_peers,
            Err(e) => {
                error!(target: "consensus::authority::utxo_syncer::start_task", "Error getting receiver handle = {:?}", e);
                panic!("Error getting receiver handle");
            }
        };

        // get peer messages receiver
        let mut peer_messages_rx = match self.get_peer_messages_rx().await {
            Ok(peer_messages_rx) => peer_messages_rx,
            Err(e) => {
                error!(target: "consensus::authority::utxo_syncer::start_task", "Error getting peer messages receiver = {:?}", e);
                panic!("Error getting peer messages receiver = {:?}", e);
            }
        };

        // receive over a channel message from other peers
        while let Some((peerid, msg)) = peer_messages_rx.recv().await {
            match msg {
                PeerMessageResponse::Pbft(_) => {
                    // Nothing to do for pbft related messages. Does are handled by the frost
                    // task
                }
                PeerMessageResponse::Dkg(_) => {
                    // Nothing to do for dkg related messages. Does are handled by the frost
                    // task
                }
                PeerMessageResponse::Signing(_) => {
                    // Nothing to do for signing related messages. Does are handled by the frost
                    // task
                }
                PeerMessageResponse::Healthcheck(_) => {
                    // Nothing to do for healthcheck related messages. Does are handled by the frost
                    // task
                }
                PeerMessageResponse::Utxo(mut response) => {
                    // check target must be us, sender must be some authority member
                    match connected_peers
                        .get(&peerid)
                        .as_ref()
                        .map(|&peer| peer.peer_commands_tx.as_ref())
                        .flatten()
                    {
                        Some(peer_sender_handle) => {
                            let serialized_compressed_utxo_set = match self
                                .get_serialized_compressed_utxo_set()
                                .await
                            {
                                Ok(serialized_compressed_utxo_set) => {
                                    serialized_compressed_utxo_set
                                }
                                Err(e) => {
                                    error!(target: "consensus::authority::utxo_syncer::start_task", "Error getting serialized compressed utxo set: {:?}", e);
                                    continue;
                                }
                            };
                            if serialized_compressed_utxo_set.is_empty() {
                                warn!(target: "consensus::authority::utxo_syncer::start_task", "Received empty utxo set from database");
                                continue;
                            }
                            response.data = serialized_compressed_utxo_set;
                            if let Err(e) = peer_sender_handle.send(FrostPeerCommand::PeerMessage(
                                PeerMessageResponse::Utxo(response),
                            )) {
                                error!(target: "consensus::authority::utxo_syncer::start_task", "Error sending utxo set message to a peer: {:?}", e);
                                continue;
                            }
                        }
                        None => {
                            warn!(target: "consensus::authority::utxo_syncer::start_task", "Unable to get peer sender handle");
                            continue;
                        }
                    }
                }
            }
        }
    }
}

impl<ToFrostMan> std::fmt::Debug for UtxoSyncTask<ToFrostMan>
where
    ToFrostMan: ToFrostManager + Clone,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UtxoSyncTask").finish_non_exhaustive()
    }
}
