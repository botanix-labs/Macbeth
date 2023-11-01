use std::sync::Arc;
use reth_primitives::{SealedHeader, Header};
use tokio::sync::RwLock;

use botanix_lib::extra_data_header::ExtraDataHeader;

#[derive(Debug, Clone, Default)]
pub(crate) struct PerBlockConstants {
    inner: Arc<RwLock<PerBlockConstantsInner>>,
}

#[derive(Default, Debug)]
pub(crate) struct PerBlockConstantsInner {
    /// The block number of the current block
    pub block_number: u64,
    
    /// Number of signers in the current epoch
    pub signer_count: u32,

    /// Zero-based index of the block signer in the sorted list of current authorized signers.
    pub signer_index: usize,

    /// Number of consecutive blocks of which a signer can only sign 1
    /// To ensure malicious signers (loss of signing key) cannot wreck havoc in the network, each singer is allowed to sign maximum one out of SIGNER_LIMIT consecutive blocks.
    /// The order is not fixed, but in-turn signing weighs more (DIFF_INTURN) than out of turn one (DIFF_NOTURN).
    pub signer_limit: u32,
}

impl PerBlockConstants {
    // TODO Remove unwrap
    fn new(last_header: SealedHeader, authority_pubkey: secp256k1::PublicKey) -> Self {
        let (header, best_hash) = last_header.split();
        // remove unwrap
        let extra_data_header = ExtraDataHeader::deserialize(last_header.extra_data.as_slice()).unwrap();
        let signer_count = extra_data_header.authority_signers.len();
        // TODO: needs access to current nodes public signing key
        let signer_index  = extra_data_header.authority_signers.iter().position(|&x| x == authority_pubkey).unwrap();

        // calculate the signer limit as `floor(signer_count / 2) + 1)`
        // Number of consecutive blocks out of which a signer may only sign one. 
        let signer_limit: u32 = ( signer_count/ 2).floor() + 1;

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
