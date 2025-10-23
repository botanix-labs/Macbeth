use botanix_tem::{
    foundation::{
        bitcoin::{BlockHash, OutPoint, Txid},
        OnchainHeaderEntry, OnchainUtxoEntry, ProposalEntry, UnassignedEntry,
    },
    validation::pegout::PegoutId,
};
use reth_storage_errors::provider::ProviderResult;

#[auto_impl::auto_impl(&, Arc, Box)]
pub trait FoundationLayerReader: Send + Sync {
    fn get_unassigned_pegout(&self, id: PegoutId) -> ProviderResult<Option<UnassignedEntry>>;
    fn get_onchain_utxo(&self, utxo: OutPoint) -> ProviderResult<Option<OnchainUtxoEntry>>;
    fn get_onchain_header(&self, header: BlockHash) -> ProviderResult<Option<OnchainHeaderEntry>>;
    fn get_pegout_proposal(&self, txid: Txid) -> ProviderResult<Option<ProposalEntry>>;
    // TODO: Rename those methods (incl. Table names!), should not contain "*foundation*".
    fn get_foundation_commitment(&self, key: [u8; 32]) -> ProviderResult<Option<Vec<u8>>>;
    fn get_foundation_commitment_root(&self) -> ProviderResult<Option<[u8; 32]>>;
}

#[auto_impl::auto_impl(&, Arc, Box)]
pub trait FoundationLayerWriter: Send + Sync {
    fn insert_unassigned_pegout(&self, id: PegoutId, entry: UnassignedEntry) -> ProviderResult<()>;
    fn remove_unassigned_pegout(&self, id: PegoutId) -> ProviderResult<bool>;
    fn insert_onchain_utxo(&self, utxo: OutPoint, entry: OnchainUtxoEntry) -> ProviderResult<()>;
    fn remove_onchain_utxo(&self, utxo: OutPoint) -> ProviderResult<bool>;
    fn insert_onchain_header(
        &self,
        header: BlockHash,
        entry: OnchainHeaderEntry,
    ) -> ProviderResult<()>;
    fn remove_onchain_header(&self, header: BlockHash) -> ProviderResult<bool>;
    fn insert_pegout_proposal(&self, txid: Txid, entry: ProposalEntry) -> ProviderResult<()>;
    fn remove_pegout_proposal(&self, txid: Txid) -> ProviderResult<bool>;
    fn insert_foundation_commitment(&self, key: [u8; 32], value: Vec<u8>) -> ProviderResult<()>;
    fn remove_foundation_commitment(&self, key: [u8; 32]) -> ProviderResult<bool>;
    fn insert_foundation_commitment_root(&self, root: [u8; 32]) -> ProviderResult<()>;
}
