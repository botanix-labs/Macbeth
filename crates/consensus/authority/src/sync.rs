use crate::engine_util;
use futures_util::StreamExt;

use reth_node_api::EngineTypes;
use reth_primitives::revm_primitives::FixedBytes;

use reth_network::NetworkEvent;

use reth_beacon_consensus::BeaconEngineMessage;
use tokio::sync::mpsc::UnboundedSender;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, error, info};

pub struct SyncController<Engine: EngineTypes> {
    network_event_listener: UnboundedReceiverStream<NetworkEvent>,
    peer_id: FixedBytes<64>,
    to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
}

impl<Engine> SyncController<Engine>
where
Engine: EngineTypes + 'static,
{
    pub fn new(
        network_event_listener: UnboundedReceiverStream<NetworkEvent>,
        peer_id: FixedBytes<64>,
        to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
    ) -> Self {
        Self { network_event_listener, peer_id, to_engine }
    }

    pub async fn start_task(&mut self) {
        loop {
            while let Some(event) = self.network_event_listener.next().await {
                if let NetworkEvent::SessionEstablished { peer_id, status, .. } = event {
                    let blockhash = status.blockhash;
                    if peer_id == self.peer_id {
                        debug!(target: "consensus::authority", "Ignoring session established event from self");
                        return;
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
                            //TODO(armins) If we cannot talk to the engine sould we panic?
                            return;
                        }
                    }
                }
            }
            // Should never get here
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
    async fn test_start_task() {
        // create network stream
        let (network_tx, rx) = mpsc::unbounded_channel::<NetworkEvent>();
        let network_stream = UnboundedReceiverStream::new(rx);
        let peer_id = PeerId::random();
        let (engine_tx, mut engine_rx) = mpsc::unbounded_channel::<BeaconEngineMessage>();

        // intialize the SyncController
        let mut sync_controller = SyncController::new(network_stream, peer_id, engine_tx);

        // spawn start_task
        let handle = tokio::spawn(async move {
            sync_controller.start_task().await;
        });

        // Create session established network event
        let status = Status::default();
        let blockhash = status.blockhash;
        let capabilities: Capabilities = vec![Capability::new_static("eth", 66)].into();
        let (tx, _rx) = mpsc::channel::<PeerRequest>(1);

        let network_event = NetworkEvent::SessionEstablished {
            peer_id: PeerId::random(),
            status: Default::default(),
            remote_addr: std::net::SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 0)),
            client_version: Arc::from(""),
            capabilities: Arc::from(capabilities),
            messages: PeerRequestSender::new(peer_id, tx),
            version: EthVersion::Eth66,
        };

        // send network event
        network_tx.send(network_event.clone()).unwrap();

        // wait for task to run
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Assert that the message with peer tip was sent
        if let BeaconEngineMessage::ForkchoiceUpdated { state, .. } = engine_rx.try_recv().unwrap()
        {
            assert_eq!(state.head_block_hash, blockhash);
        }
    }
}
