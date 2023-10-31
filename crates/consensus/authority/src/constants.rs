#[derive(Debug, Clone, Default)]
pub(crate) struct PerBlockConstants {
    inner: Arc<RwLock<PerBlockConstantsInner>>,
}

#[derive(Defualt, Debuf)]
pub(crate) struct PerBlockConstantsInner {
    /// The block number of the current block
    pub block_number: u64;
    
    /// Number of signers in the current epoch
    pub signer_count: u32;

    /// Zero-based index of the block signer in the sorted list of current authorized signers.
    pub signer_index: usize;

    /// Number of consecutive blocks of which a signer can only sign 1
    pub signer_limit: u32;
}

impl PerBlockConstants {
    fn new(last_header: SealedHeader) -> Self {
        let (header, best_hash) = header.split();
         
        // extract just signer bytes list from `extra_data` field, and chunk into 32 byte chunks
        let mut signer_data = header.extra_data[32, header.extra_data.len() - 65];
        signer_data = signer_data.chunk(32).collect<Vec<&[u8]>>();

        let signer_count = signer_data.len(); 

        // TODO: needs access to current nodes public signing key
        let signer_index: usize = todo!();

        /// calculate the signer limit as `floor(signer_count / 2) + 1)`
        let signer_limit: u32 = (signer_data.len() / 2).floor() + 1;

        let mut constants = PerBlockConstantsInner {
            block_number: best_hash,
            signer_count,
            signer_index,
            signer_limit 
        };

        Self { inner: Arc::new(RwLock::new(constants))}
    }
}

impl PerBlockConstantsInner {
    pub(crate) fn new_block(&mut self, mut header: Header) {
        header.number = self.best_block + 1
    }
}
