use futures_util::Future;
use reth_interfaces::p2p::{headers::client::HeadersClient, bodies::client::BodiesClient};
use reth_network::NetworkHandle;
use reth_primitives::IntoRecoveredTransaction;
use reth_transaction_pool::{ValidPoolTransaction, TransactionPool};
use tokio::time::{Interval, Sleep};
use std::{pin::Pin, task::{Context, Poll}, sync::Arc, time::Duration};

use crate::builder::Storage;


#[derive(Debug)]
pub struct EpochManager <Client, Pool: TransactionPool>
where 
    Client: HeadersClient + BodiesClient,
{
    /// access to storage to fetch headers
    storage: Storage,

    /// pollable interval to lock nodes proposing for a min time defined by `BLOCK_PERIOD`
    proposal_interval: Interval,

    random_delay: Option<Pin<Box<Sleep>>>,

    /// The block number of the current block
    pub block_number: u64,
    
    /// Number of signers in the current epoch
    pub signer_count: u32,

    /// Zero-based index of the block signer in the sorted list of current authorized signers.
    pub signer_index: usize,

    /// Number of consecutive blocks of which a signer can only sign 1
    pub signer_limit: u32,

    client: Client,
	
	pool: Pool,
}

impl<Client, Pool: TransactionPool> EpochManager<Client, Pool> 
where
    Client: HeadersClient + BodiesClient,
{
    pub fn new(storage: Storage, network: NetworkHandle) -> Self {
        // get the header for the best known block
        let header = storage.headers.get(&storage.best_block);

        if header.extra_data.len() > 97 {
            //TODO: handle when there are signers in the `extra_data` field
        } else {
            // TODO: handle when there are no signers
            todo!()
        }
    }


}

impl<Client, Pool> Future for EpochManager<Client, Pool>
where
    Client: HeadersClient + BodiesClient,
    Pool: TransactionPool + Unpin + 'static,
    <Pool as TransactionPool>::Transaction: IntoRecoveredTransaction,
{
	type Output = ();

	fn poll(
		&mut self,
		pool: &Pool,
		cx: &mut Context<'_>,
	) -> Poll<Self::Output>
	{
		let is_inturn = self.block_number % self.signer_count == self.signer_index;
		match self.random_delay {
			Some(ref mut delay) => {
				if delay.poll(cx).is_ready() {
					self.random_delay = None;
					// drain pool
					return Poll::Ready(pool.best_transactions().collect())
				}
				Poll::Pending
			}
			None => {
				if self.proposal_interval.poll_tick(cx).is_ready() && is_inturn {
					return Poll::Ready(pool.best_transactions().collect())
				} else if self.proposal_interval.poll_tix(cx).is_ready() && !is_inturn {
					// NOTE: verify if network can/should be handled here or in the main task
					// TODO: check network handle for gossiped block
					// TODO: set gossiped block header in storage or...
					// TODO: if `None` do the following 
					let duration = Duration::from_secs(6);
					self.random_delay = Some(tokio::time::sleep(duration));
					Poll::Pending
				} else {
					Poll::Pending
				}
			}

		}
		// inturn
	}
}
