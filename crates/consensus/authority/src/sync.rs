use crate::task::BlockProductionTask;
use futures_util::StreamExt;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_network::{NetworkEvent, NetworkEvents};

use reth_provider::{CanonChainTracker, StateProviderFactory};
use reth_rpc_types::engine::ForkchoiceState;
use reth_transaction_pool::TransactionPool;
use tokio::sync::oneshot;

use tracing::{debug, info};

impl<Client, Pool: TransactionPool> BlockProductionTask<Client, Pool>
where
    Client: StateProviderFactory + CanonChainTracker + Clone + 'static,
    Pool: TransactionPool,
{
    pub(crate) async fn try_sync_peer_tip(&self) {
        let mut network_event_listener = self.network_handle.event_listener();
        let to_engine = self.to_engine.clone();
        let local_peer_id = self.network_handle.peer_id().clone();
        if let Some(event) = network_event_listener.next().await {
            if let NetworkEvent::SessionEstablished { peer_id, status, .. } = event {
                let blockhash = status.blockhash;
                if peer_id == local_peer_id {
                    debug!(target: "consensus::authority", "Ignoring session established event from self");
                    return
                }
                let state = ForkchoiceState {
                    head_block_hash: blockhash,
                    finalized_block_hash: blockhash,
                    safe_block_hash: blockhash,
                };

                info!(target: "consensus::authority", "Sending fork choice update with new tip {} from peer {}", blockhash, peer_id
                );
                let (tx, _rx) = oneshot::channel();
                let _ = to_engine.send(BeaconEngineMessage::ForkchoiceUpdated {
                    state,
                    payload_attrs: None,
                    tx,
                });

                // TODO (scott) use util function to handle _rx messages
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
