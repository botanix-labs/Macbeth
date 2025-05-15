use std::fmt::{Debug, Formatter};
use tendermint_proto::abci::{
    RequestApplySnapshotChunk, RequestProcessProposal, ResponseLoadSnapshotChunk,
    ResponsePrepareProposal,
};

struct TruncatedSlice<'a, T> {
    inner: &'a [T],
    max_len: usize,
}

impl<'a, T> TruncatedSlice<'a, T> {
    fn new(slice: &'a [T], max_len: usize) -> Self {
        Self { inner: slice, max_len }
    }
}

impl<T> Debug for TruncatedSlice<'_, T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let dbg_len = std::cmp::min(self.inner.len(), self.max_len);
        let is_truncated = dbg_len < self.inner.len();

        f.write_str("[")?;

        if self.max_len == 0 {
            f.write_fmt(format_args!(" {} items ", self.inner.len()))?;
        } else {
            for (i, entry) in self.inner[..dbg_len].iter().enumerate() {
                f.write_fmt(format_args!("{:?}", &entry))?;
                if i != dbg_len - 1 {
                    f.write_str(", ")?;
                }
            }

            if is_truncated {
                f.write_fmt(format_args!(", ...({} more)", self.inner.len() - dbg_len))?;
            }
        }

        f.write_str("]")
    }
}

/// Truncated debug implementation for [RequestProcessProposal].
/// It uses [TruncatedSlice] to limit the number of transactions displayed in the debug output.
pub(crate) struct RequestProcessProposalTruncatedDebug<'a>(pub &'a RequestProcessProposal);

impl Debug for RequestProcessProposalTruncatedDebug<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let txs = TruncatedSlice::new(self.0.txs.as_slice(), 0);

        f.debug_struct("RequestProcessProposal")
            .field("txs", &txs)
            .field("proposed_last_commit", &self.0.proposed_last_commit)
            .field("misbehavior", &self.0.misbehavior)
            .field("hash", &self.0.hash)
            .field("height", &self.0.height)
            .field("time", &self.0.time)
            .field("next_validators_hash", &self.0.next_validators_hash)
            .field("proposer_address", &self.0.proposer_address)
            .finish()
    }
}

/// Truncated debug implementation for [ResponsePrepareProposal].
/// It uses [TruncatedSlice] to limit the number of transactions displayed in the debug output.
pub(crate) struct ResponsePrepareProposalTruncatedDebug<'a>(pub &'a ResponsePrepareProposal);

impl Debug for ResponsePrepareProposalTruncatedDebug<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let txs = TruncatedSlice::new(self.0.txs.as_slice(), 0);

        f.debug_struct("ResponsePrepareProposal").field("txs", &txs).finish()
    }
}

/// Truncated debug implementation for [RequestApplySnapshotChunk].
/// It uses [TruncatedSlice] to limit the number of chunk bytes displayed in the debug output.
pub(crate) struct RequestApplySnapshotChunkTruncatedDebug<'a>(pub &'a RequestApplySnapshotChunk);

impl Debug for RequestApplySnapshotChunkTruncatedDebug<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let chunk = TruncatedSlice::new(self.0.chunk.as_ref(), 3);

        f.debug_struct("RequestApplySnapshotChunk")
            .field("index", &self.0.index)
            .field("chunk", &chunk)
            .field("sender", &self.0.sender)
            .finish()
    }
}

/// Truncated debug implementation for [ResponseLoadSnapshotChunk].
/// It uses [TruncatedSlice] to limit the number of chunk bytes displayed in the debug output.
pub(crate) struct ResponseLoadSnapshotChunkTruncatedDebug<'a>(pub &'a ResponseLoadSnapshotChunk);

impl Debug for ResponseLoadSnapshotChunkTruncatedDebug<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let chunk = TruncatedSlice::new(self.0.chunk.as_ref(), 3);

        f.debug_struct("ResponseLoadSnapshotChunk").field("chunk", &chunk).finish()
    }
}

pub(crate) struct RequestFinalizeBlockTruncatedDebug<'a>(
    pub &'a tendermint_proto::abci::RequestFinalizeBlock,
);

impl Debug for RequestFinalizeBlockTruncatedDebug<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let txs = TruncatedSlice::new(self.0.txs.as_slice(), 0);

        f.debug_struct("RequestFinalizeBlock")
            .field("txs", &txs)
            .field("decided_last_commit", &self.0.decided_last_commit)
            .field("misbehavior", &self.0.misbehavior)
            .field("hash", &self.0.hash)
            .field("height", &self.0.height)
            .field("time", &self.0.time)
            .field("next_validators_hash", &self.0.next_validators_hash)
            .field("proposer_address", &self.0.proposer_address)
            .finish()
    }
}
