use reth_eth_wire::NewBlock;
use reth_network::{NetworkHandle, NetworkProtocols, PeerRequest};
use reth_network_api::{Peers, PeersInfo};

use reth_primitives::{Block, B256};
use ruint::Uint;
use tracing::info;

use client::BtcServerClient;

pub struct FrostTask {
    /// BTC Server client
    pub(crate) btc_server: BtcServerClient<tonic::transport::Channel>,
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    // TODO: FrostHandle
}

impl FrostTask {
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        btc_server: BtcServerClient<tonic::transport::Channel>,
        network_handle: NetworkHandle,
    ) -> Self {
        Self { btc_server, network_handle }
    }

    pub async fn start_task(&mut self) -> () {
        // start the emiting loop
        let mut counter = 0;
        loop {
            info!("Sending Frost Message to all peers");
            //let _ = self.network_handle.send_frost_msg(msg).await;
            //self.network_handle.send_request(peer_id, request)

            /*
            let all_piers = self.network_handle.num_connected_peers();
            info!("MMMMMMMMMMMMMMM {:?}", all_piers);

            let all_conn_peers = self.network_handle.get_all_peers().await.unwrap();
            info!("PPPPPPPPPPPPPP {:?}", all_conn_peers.len());

            for peer in all_conn_peers {
                let msg = format!("Hello Frost-{}", counter).as_bytes().to_vec();
                let _ = self.network_handle.send_frost_msg(peer.remote_id, msg);
            }
            */

            //self.network_handle.send_request(peer_id, PeerRequest::)
            /*
            let empty_block = Block::default();
            let hash = empty_block.mix_hash.clone();
            let new_block = NewBlock { block: empty_block, td: Uint::ZERO };
            self.network_handle.announce_block(new_block, hash);
            */

            // TODO : add the logic
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            counter += 1;
        }
    }
}

impl std::fmt::Debug for FrostTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrostTask").finish_non_exhaustive()
    }
}
