//! reth P2P networking.
//!
//! Ethereum's networking protocol is specified in [devp2p](https://github.com/ethereum/devp2p).
//!
//! In order for a node to join the ethereum p2p network it needs to know what nodes are already
//! part of that network. This includes public identities (public key) and addresses (where to reach
//! them).
//!
//! ## Bird's Eye View
//!
//! See also diagram in [`NetworkManager`]
//!
//! The `Network` is made up of several, separate tasks:
//!
//!    - `Transactions Task`: is a spawned
//!      [`TransactionsManager`](crate::transactions::TransactionsManager) future that:
//!
//!        * Responds to incoming transaction related requests
//!        * Requests missing transactions from the `Network`
//!        * Broadcasts new transactions received from the
//!          [`TransactionPool`](reth_transaction_pool::TransactionPool) over the `Network`
//!
//!    - `ETH request Task`: is a spawned
//!      [`EthRequestHandler`](crate::eth_requests::EthRequestHandler) future that:
//!
//!        * Responds to incoming ETH related requests: `Headers`, `Bodies`
//!
//!    - `Discovery Task`: is a spawned [`Discv4`](reth_discv4::Discv4) future that handles peer
//!      discovery and emits new peers to the `Network`
//!
//!    - [`NetworkManager`] task advances the state of the `Network`, which includes:
//!
//!        * Initiating new _outgoing_ connections to discovered peers
//!        * Handling _incoming_ TCP connections from peers
//!        * Peer management
//!        * Route requests:
//!             - from remote peers to corresponding tasks
//!             - from local to remote peers
//!
//! ## Usage
//!
//! ### Configure and launch a standalone network
//!
//!
//! # Feature Flags
//!
//! - `serde` (default): Enable serde support for configuration types.
//! - `test-utils`: Various utilities helpful for writing tests

// Add back launch docs after upstream merge
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![allow(unreachable_pub)]
#![cfg_attr(not(test), allow(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

#[cfg(any(test, feature = "test-utils"))]
/// Common helpers for network testing.
pub mod test_utils;

pub mod cache;
pub mod config;
pub mod error;
pub mod eth_requests;
pub mod frost;
pub mod import;
pub mod message;
pub mod peers;
pub mod protocol;
pub mod transactions;

mod budget;
mod builder;
mod discovery;
mod fetch;
mod flattened_response;
mod listener;
mod manager;
mod metrics;
mod network;
mod session;
mod state;
mod swarm;
mod trusted_peers_resolver;

pub use reth_eth_wire::{DisconnectReason, HelloMessageWithProtocols};
//pub use reth_eth_wire_types::{primitives, EthNetworkPrimitives, NetworkPrimitives};
pub use reth_network_api::{
    events, BlockDownloaderProvider, DiscoveredEvent, DiscoveryEvent, NetworkEvent,
    NetworkEventListenerProvider, NetworkInfo, PeerRequest, PeerRequestSender, Peers, PeersInfo,
};
pub use reth_network_p2p::sync::{NetworkSyncUpdater, SyncState};
pub use reth_network_types::{PeersConfig, SessionsConfig};
pub use session::{
    ActiveSessionHandle, ActiveSessionMessage, Direction, EthRlpxConnection, PeerInfo,
    PendingSessionEvent, PendingSessionHandle, PendingSessionHandshakeError, SessionCommand,
    SessionEvent, SessionId, SessionManager,
};

pub use builder::NetworkBuilder;
pub use config::{NetworkConfig, NetworkConfigBuilder};
pub use discovery::Discovery;
pub use fetch::FetchClient;
pub use flattened_response::FlattenedResponse;
pub use manager::NetworkManager;
pub use metrics::TxTypesCounter;
pub use network::{NetworkHandle, NetworkProtocols};
pub use swarm::NetworkConnectionState;
pub use transactions::{FilterAnnouncement, MessageFilter, ValidateTx68};

/// re-export p2p interfaces
pub use reth_network_p2p as p2p;

/// re-export types crates
pub mod types {
    //pub use reth_eth_wire_types::*;
    pub use reth_network_types::*;
}

use aquamarine as _;

use smallvec as _;
