//! Builder support for configuring the entire setup.

use crate::{
    eth_requests::EthRequestHandler,
    frost::manager::{FrostConfig, FrostManager},
    transactions::{TransactionsManager, TransactionsManagerConfig},
    NetworkHandle, NetworkManager,
};
use reth_transaction_pool::TransactionPool;
use tokio::sync::mpsc;

/// We set the max channel capacity of the EthRequestHandler to 256
/// 256 requests with malicious 10MB body requests is 2.6GB which can be absorbed by the node.
pub(crate) const ETH_REQUEST_CHANNEL_CAPACITY: usize = 256;

/// A builder that can configure all components of the network.
#[allow(missing_debug_implementations)]
pub struct NetworkBuilder<C, Tx, Eth> {
    pub(crate) network: NetworkManager<C>,
    pub(crate) transactions: Tx,
    pub(crate) request_handler: Eth,
    pub(crate) frost_manager: Option<FrostManager>,
}

// === impl NetworkBuilder ===

impl<C, Tx, Eth> NetworkBuilder<C, Tx, Eth> {
    /// Consumes the type and returns all fields.
    pub fn split(self) -> (NetworkManager<C>, Tx, Eth, Option<FrostManager>) {
        let NetworkBuilder { network, transactions, request_handler, frost_manager } = self;
        (network, transactions, request_handler, frost_manager)
    }

    /// Returns the network manager.
    pub fn network(&self) -> &NetworkManager<C> {
        &self.network
    }

    /// Returns the mutable network manager.
    pub fn network_mut(&mut self) -> &mut NetworkManager<C> {
        &mut self.network
    }

    /// Returns the handle to the network.
    pub fn handle(&self) -> NetworkHandle {
        self.network.handle().clone()
    }

    /// Consumes the type and returns all fields and also return a [`NetworkHandle`].
    pub fn split_with_handle(
        self,
    ) -> (NetworkHandle, NetworkManager<C>, Tx, Eth, Option<FrostManager>) {
        let NetworkBuilder { network, transactions, request_handler, frost_manager } = self;
        let handle = network.handle().clone();
        (handle, network, transactions, request_handler, frost_manager)
    }

    /// Creates a new [`FrostManager`] and wires it to the network.
    pub fn frost(self, frost_config: Option<FrostConfig>) -> NetworkBuilder<C, Tx, Eth> {
        if frost_config.is_none() {
            self
        } else {
            let NetworkBuilder { mut network, request_handler, transactions, .. } = self;
            let (tx, rx) = mpsc::unbounded_channel();
            network.set_frost_manager(tx);
            let handle = network.handle().clone();
            let frost_manager =
                FrostManager::new(frost_config.expect("frost config exists"), handle, rx);
            NetworkBuilder {
                network,
                request_handler,
                transactions,
                frost_manager: Some(frost_manager),
            }
        }
    }

    /// Creates a new [`TransactionsManager`] and wires it to the network.
    pub fn transactions<Pool: TransactionPool>(
        self,
        pool: Pool,
        transactions_manager_config: TransactionsManagerConfig,
    ) -> NetworkBuilder<C, TransactionsManager<Pool>, Eth> {
        let NetworkBuilder { mut network, request_handler, frost_manager, .. } = self;
        let (tx, rx) = mpsc::unbounded_channel();
        network.set_transactions(tx);
        let handle = network.handle().clone();
        let transactions = TransactionsManager::new(handle, pool, rx, transactions_manager_config);
        NetworkBuilder { network, request_handler, transactions, frost_manager }
    }

    /// Creates a new [`EthRequestHandler`] and wires it to the network.
    pub fn request_handler<Client>(
        self,
        client: Client,
    ) -> NetworkBuilder<C, Tx, EthRequestHandler<Client>> {
        let NetworkBuilder { mut network, transactions, frost_manager, .. } = self;
        let (tx, rx) = mpsc::channel(ETH_REQUEST_CHANNEL_CAPACITY);
        network.set_eth_request_handler(tx);
        let peers = network.handle().peers_handle().clone();
        let request_handler = EthRequestHandler::new(client, peers, rx);
        NetworkBuilder { network, request_handler, transactions, frost_manager }
    }
}
