use crate::{
    error::{BackoffKind, SessionError},
    peers::{
        ReputationChangeWeights, DEFAULT_MAX_CONCURRENT_DIALS, DEFAULT_MAX_PEERS_INBOUND,
        DEFAULT_MAX_PEERS_OUTBOUND,
    },
    session::{Direction, PendingSessionHandshakeError},
    SessionManager,
};
use futures::StreamExt;
use reth_eth_wire::{errors::EthStreamError, DisconnectReason};
use reth_net_common::ban_list::BanList;
use reth_network_api::{PeerKind, ReputationChangeKind};
use reth_primitives::{ForkId, NodeRecord, PeerId};
use std::{
    collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
    fmt::Display,
    io::{self, ErrorKind},
    net::{IpAddr, SocketAddr},
    path::Path,
    task::{Context, Poll},
    time::Duration,
};
use thiserror::Error;
use tokio::{
    sync::{
        mpsc,
        mpsc::{UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    time::{Instant, Interval},
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{info, trace};

use super::{protocol::PingPongProtoHandler, Command, ProtocolEvent, ProtocolState};

#[derive(Clone, Debug)]
pub struct FrostHandle {
    manager_tx: mpsc::UnboundedSender<FrostCommand>,
}

// === impl FrostHandle ===

impl FrostHandle {
    fn send(&self, cmd: FrostCommand) {
        let _ = self.manager_tx.send(cmd);
    }
}

#[derive(Debug)]
pub struct FrostManager {
    /// Copy of the sender half, so new [`FrostManager`] can be created on demand.
    manager_tx: mpsc::UnboundedSender<FrostCommand>,
    /// Receiver half of the command channel.
    handle_rx: UnboundedReceiverStream<FrostCommand>,
    ///
    //from_peers: UnboundedReceiver<ProtocolEvent>,
    peer0_conn: Option<UnboundedSender<Command>>,
}

impl FrostManager {
    /// Create a new instance with the given config
    pub fn new(config: FrostConfig, session: &mut SessionManager) -> Self {
        let FrostConfig { some_bool } = config;
        let (manager_tx, handle_rx) = mpsc::unbounded_channel();

        let (tx, mut from_peers) = mpsc::unbounded_channel();
        let protocol_state = ProtocolState { events: tx };
        let protocol_handler = PingPongProtoHandler { state: protocol_state };
        session.add_rlpx_sub_protocol(protocol_handler);

        tokio::spawn(async move {
            loop {
                if let Some(protocol_event) = from_peers.recv().await {
                    info!("PPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPP");
                    match protocol_event {
                        ProtocolEvent::Established { direction: _, peer_id, to_connection } => {}
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });

        Self { peer0_conn: None, manager_tx, handle_rx: UnboundedReceiverStream::new(handle_rx) }
    }

    /// Returns a new [`FrostHandle`] that can send commands to this type.
    pub(crate) fn handle(&self) -> FrostHandle {
        FrostHandle { manager_tx: self.manager_tx.clone() }
    }
}

// impl Default for FrostManager {
//     fn default() -> Self {
//         FrostManager::new(Default::default())
//     }
// }

/// Commands the [`PeersManager`] listens for.
#[derive(Debug)]
pub(crate) enum FrostCommand {
    /// Command for manually add
    Add(PeerId, SocketAddr),
    /// Get node information on all peers
    GetPeers(oneshot::Sender<Vec<NodeRecord>>),
}

/// Config type for initiating a [`PeersManager`] instance.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct FrostConfig {
    some_bool: bool,
}

impl Default for FrostConfig {
    fn default() -> Self {
        Self { some_bool: false }
    }
}
