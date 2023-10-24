#[derive(Debug)]
pub struct EpochManager {
    /// access to storage to fetch headers
    storage: Storage,

    /// pollable interval to lock nodes proposing for a min time defined by `BLOCK_PERIOD`
    proposal_interval: Interval,

    random_delay: Option<Pin<Box<Sleep>>>,

    /// The block number of the current block
    pub block_number: u64;
    
    /// Number of signers in the current epoch
    pub signer_count: u32;

    /// Zero-based index of the block signer in the sorted list of current authorized signers.
    pub signer_index: usize;

    /// Number of consecutive blocks of which a signer can only sign 1
    pub signer_limit: u32;
}

impl EpochManager {
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


    pub(crate) fn poll<Pool>(
        &mut self,
        pool: &Pool,
        cx: &mut Context<'_>,
    ) -> Pool<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>
    where
        Pool: TransactionPool,
    {
        let is_inturn = (self.block_number % self.signer_count == self.signer_index);
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
                if proposal_interval.poll_tick(cx).is_ready() && is_inturn {
                    return Poll::Ready(pool.best_transactions().collect())
                } else if proposal_interval.poll_tix(cx).is_ready() && !is_inturn {
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
