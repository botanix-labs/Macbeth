use crate::{
    models::{PeerID, UuidID, WalletStateSyncRecord},
    provider::{
        database::provider::BotanixDatabaseProvider, WalletStateSyncReader, WalletStateSyncWriter,
    },
    tables,
};
use reth_db_api::{
    cursor::DbCursorRO,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::Bytes;
use reth_storage_errors::provider::ProviderResult;

impl<TX: DbTxMut + DbTx> WalletStateSyncWriter for BotanixDatabaseProvider<TX>
where
    Self: WalletStateSyncReader,
{
    fn create_new_state_sync_record(
        &self,
        uuid: UuidID,
        peer_id: PeerID,
        chunks_count: u64,
        data: Option<Vec<(u64, Bytes)>>,
    ) -> ProviderResult<PeerID> {
        let wallet_state_sync_record =
            WalletStateSyncRecord::new(peer_id, uuid, chunks_count, data);
        self.tx.put::<tables::WalletStateSyncs>(peer_id, wallet_state_sync_record)?;
        Ok(peer_id)
    }

    fn append_data_to_state_sync_record(
        &self,
        peer_id: PeerID,
        data: Vec<(u64, Bytes)>,
    ) -> ProviderResult<()> {
        let wallet_state_sync_record = self
            .tx
            .cursor_write::<tables::WalletStateSyncs>()?
            .seek_exact(peer_id)?
            .map(|(_, record)| record);

        if let Some(mut wallet_state_sync_record) = wallet_state_sync_record {
            for (block, data_chunk) in data {
                wallet_state_sync_record.append_data_with_block(data_chunk, block);
            }
            self.tx.put::<tables::WalletStateSyncs>(peer_id, wallet_state_sync_record)?;
        }
        Ok(())
    }

    fn remove_state_sync_record_per_peer_id(&self, peer_id: PeerID) -> ProviderResult<()> {
        self.remove::<tables::WalletStateSyncs>(peer_id..=peer_id)?;
        Ok(())
    }

    fn remove_all_state_sync_records(&self) -> ProviderResult<()> {
        let state_sync_records = self.get_state_sync_record_peer_ids()?;
        if state_sync_records.is_empty() {
            return Ok(());
        }
        for state_sync_record in state_sync_records {
            self.remove::<tables::WalletStateSyncs>(state_sync_record..=state_sync_record)?;
        }
        Ok(())
    }
}
