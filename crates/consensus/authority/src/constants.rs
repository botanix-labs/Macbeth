use reth_primitives::{Header, SealedHeader};
use std::sync::Arc;
use tokio::sync::RwLock;

use botanix_lib::extra_data_header::ExtraDataHeader;

#[derive(Debug, Clone, Default)]
pub(crate) struct PerBlockConstants {
    inner: Arc<RwLock<PerBlockConstantsInner>>,
}

#[derive(Default, Debug)]
pub(crate) struct PerBlockConstantsInner {
    /// The block number of the current block
    pub(crate) block_number: u64,

    /// Number of signers in the current epoch
    pub(crate) signer_count: u32,

    /// Zero-based index of the block signer in the sorted list of current authorized signers.
    pub(crate) signer_index: usize,

    /// Number of consecutive blocks of which a signer can only sign 1
    /// To ensure malicious signers (loss of signing key) cannot wreck havoc in the network, each
    /// singer is allowed to sign maximum one out of SIGNER_LIMIT consecutive blocks. The order
    /// is not fixed, but in-turn signing weighs more (DIFF_INTURN) than out of turn one
    /// (DIFF_NOTURN).
    pub(crate) signer_limit: u32,
}

impl PerBlockConstants {
    // TODO Remove unwraps
    fn new(last_header: SealedHeader, authority_pubkey: secp256k1::PublicKey) -> Self {
        let (header, _) = last_header.split();
        // remove unwrap
        let extra_data_header =
            ExtraDataHeader::deserialize(header.extra_data.to_vec()).unwrap();
        let signer_count: u32 = extra_data_header.authority_signers.len() as u32;
        // TODO: needs access to current nodes public signing key
        let signer_index = extra_data_header
            .authority_signers
            .iter()
            .position(|&pk| pk == authority_pubkey)
            .unwrap();

        // calculate the signer limit as `floor(signer_count / 2) + 1)`
        // Number of consecutive blocks out of which a signer may only sign one.
        let signer_limit: u32 = (signer_count / 2) + 1;

        let constants = PerBlockConstantsInner {
            block_number: header.number,
            signer_count,
            signer_index,
            signer_limit,
        };

        Self { inner: Arc::new(RwLock::new(constants)) }
    }
}

// TODO come back to this
// impl PerBlockConstantsInner {
//     pub(crate) fn new_block(&mut self, mut header: Header) {
//         header.number = self.best_block + 1
//     }
// }
