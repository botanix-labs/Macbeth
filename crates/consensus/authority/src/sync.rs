use crate::engine_util;
use futures_util::StreamExt;

use reth_primitives::revm_primitives::FixedBytes;

use reth_network::NetworkEvent;

use reth_beacon_consensus::BeaconEngineMessage;
use tokio::sync::mpsc::UnboundedSender;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, error, info};

pub(crate) struct SyncController {
    network_event_listener: UnboundedReceiverStream<NetworkEvent>,
    peer_id: FixedBytes<64>,
    to_engine: UnboundedSender<BeaconEngineMessage>,
}

impl SyncController {
    pub(crate) fn new(
        network_event_listener: UnboundedReceiverStream<NetworkEvent>,
        peer_id: FixedBytes<64>,
        to_engine: UnboundedSender<BeaconEngineMessage>,
    ) -> Self {
        Self { network_event_listener, peer_id, to_engine }
    }

    pub(crate) async fn try_sync_peer_tip(&mut self) {
        while let Some(event) = self.network_event_listener.next().await {
            if let NetworkEvent::SessionEstablished { peer_id, status, .. } = event {
                let blockhash = status.blockhash;
                if peer_id == self.peer_id {
                    debug!(target: "consensus::authority", "Ignoring session established event from self");
                    return
                }
                match engine_util::send_fork_choice_update_payload(
                    blockhash,
                    self.to_engine.clone(),
                )
                .await
                {
                    Ok(_) => {
                        info!(target: "consensus::authority", "Sending fork choice update with new tip {} from peer {}", blockhash, peer_id);
                    }
                    Err(err) => {
                        error!(target: "consensus::authority", "Failed to send fork choice update with new tip {} from peer {}: {:?}", blockhash, peer_id, err);
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        net::{Ipv4Addr, SocketAddrV4},
        sync::Arc,
        time::Duration,
    };

    use super::*;
    use reth_beacon_consensus::BeaconEngineMessage;
    use reth_eth_wire::{
        capability::{Capabilities, Capability},
        EthVersion, Status,
    };
    use reth_network::{message::PeerRequestSender, PeerRequest};
    use reth_rpc_types::PeerId;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_sync_peer_tip() {
        // Create session established network event
        let peer_id = PeerId::random();
        let status = Status::default();
        let blockhash = status.blockhash;
        let capabilities: Capabilities = vec![Capability::new_static("eth", 66)].into();
        let (tx, _rx) = mpsc::channel::<PeerRequest>(1);
        let network_event = NetworkEvent::SessionEstablished {
            peer_id,
            status: Default::default(),
            remote_addr: std::net::SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 0)),
            client_version: Arc::from(""),
            capabilities: Arc::from(capabilities),
            messages: PeerRequestSender::new(peer_id, tx),
            version: EthVersion::Eth66,
        };

        // create network stream
        let (network_tx, rx) = mpsc::unbounded_channel::<NetworkEvent>();
        let network_stream = UnboundedReceiverStream::new(rx);

        // create beacon engine channel
        let (engine_tx, mut engine_rx) = mpsc::unbounded_channel::<BeaconEngineMessage>();

        // spawn sync_peer_tip task
        let handle = tokio::spawn(sync_peer_tip(network_stream, engine_tx, peer_id));

        // send network event
        network_tx.send(network_event.clone()).unwrap();

        // wait for task to run
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Assert that the message with peer tip was sent
        if let BeaconEngineMessage::ForkchoiceUpdated { state, .. } = engine_rx.try_recv().unwrap()
        {
            assert_eq!(state.head_block_hash, blockhash);
        }

        // Cancel the spawned task
        handle.abort();

        // TODO (scott) update test to check engine response when function is updated
    }
}
