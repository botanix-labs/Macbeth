use serde::{Deserialize, Serialize};

/// Type uniquely representing a pegout request.
#[derive(Serialize, Deserialize, Hash, Clone, Copy, PartialEq, Eq)]
pub struct PegoutId {
    /// TxHash of the botanix tx.
    pub txid: [u8; 32],
    /// Index of the log that includes the pegout request within this tx receipt.
    pub idx: u32,
}

impl PegoutId {
    pub fn new(txid: [u8; 32], idx: u32) -> PegoutId {
        PegoutId { txid, idx }
    }

    pub fn as_bytes(&self) -> [u8; 36] {
        let mut ret = [0u8; 36];
        ret[0..32].copy_from_slice(&self.txid[..]);
        ret[32..36].copy_from_slice(&self.idx.to_be_bytes()[..]);
        ret
    }

    /// Returns an error only if the byte string is not of length 36.
    #[allow(clippy::result_unit_err)]
    pub fn from_bytes(bytes: &[u8]) -> Result<PegoutId, ()> {
        if bytes.len() == 36 {
            Ok(PegoutId {
                txid: {
                    let mut buf = [0u8; 32];
                    buf.copy_from_slice(&bytes[0..32]);
                    buf
                },
                idx: {
                    let mut buf = [0u8; 4];
                    buf.copy_from_slice(&bytes[32..36]);
                    u32::from_be_bytes(buf)
                },
            })
        } else {
            Err(())
        }
    }
}

impl From<[u8; 36]> for PegoutId {
    fn from(b: [u8; 36]) -> PegoutId {
        PegoutId::from_bytes(&b[..]).expect("size is 36")
    }
}
impl std::fmt::Display for PegoutId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}:{}", self.txid, self.idx)
    }
}
impl std::fmt::Debug for PegoutId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
