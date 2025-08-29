use crate::{
    models::{PeerID, UuidID, WalletStateSyncRecord},
    provider::{
        database::provider::BotanixDatabaseProvider, WalletStateSyncReader, WalletStateSyncWriter,
    },
    tables::WalletStateSyncs,
    BotanixDatabaseProviderRW,
};
use reth_db_api::{
    cursor::DbCursorRO,
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_primitives::Bytes;
use reth_storage_errors::provider::ProviderResult;
use std::collections::{HashMap, HashSet};

impl<TX: DbTx> WalletStateSyncReader for BotanixDatabaseProvider<TX> {
    fn get_state_sync_records(&self) -> ProviderResult<Vec<WalletStateSyncRecord>> {
        Ok(self
            .tx
            .cursor_read::<WalletStateSyncs>()?
            .walk(None)?
            .collect::<Result<HashMap<_, _>, _>>()?
            .values()
            .cloned()
            .collect::<Vec<_>>())
    }

    fn get_state_sync_record_peer_ids(&self) -> ProviderResult<Vec<PeerID>> {
        Ok(self
            .tx
            .cursor_read::<WalletStateSyncs>()?
            .walk(None)?
            .collect::<Result<HashMap<_, _>, _>>()?
            .values()
            .map(|val| val.get_peer_id())
            .collect::<Vec<_>>())
    }

    fn get_state_sync_record_by_peer_id(
        &self,
        peer_id: PeerID,
    ) -> ProviderResult<Option<WalletStateSyncRecord>> {
        Ok(self
            .tx
            .cursor_read::<WalletStateSyncs>()?
            .seek_exact(peer_id)
            .ok()
            .flatten()
            .map(|x| x.1))
    }

    fn get_state_sync_records_count(&self) -> ProviderResult<usize> {
        Ok(self.tx.cursor_read::<WalletStateSyncs>()?.walk(None)?.count())
    }

    fn get_minimum_superset(
        &self,
        min_required_criterion: u64,
    ) -> ProviderResult<(bool, HashSet<(u64, Bytes)>)> {
        let already_reached_wallet_state_sync_peers = self
            .tx
            .cursor_read::<WalletStateSyncs>()?
            .walk(None)?
            .filter_map(|item| match item {
                Ok((peer_id, wallet_state_sync_record)) => {
                    if wallet_state_sync_record.get_data().len() as u64 >=
                        wallet_state_sync_record.get_chunks_count()
                    {
                        return Some((peer_id, wallet_state_sync_record));
                    }
                    None
                }
                Err(_) => None,
            })
            .collect::<HashMap<_, _>>();

        if already_reached_wallet_state_sync_peers.len() < min_required_criterion as usize {
            return Ok((false, HashSet::new()));
        }

        let synced_peers_superset = already_reached_wallet_state_sync_peers.into_iter().fold(
            HashSet::new(),
            |mut acc, (_, mut record)| {
                acc.extend(record.blocks_and_data_to_set());
                acc
            },
        );

        Ok((true, synced_peers_superset))
    }
}

impl<DB: Database> WalletStateSyncReader for BotanixDatabaseProviderRW<DB> {
    #[inline(always)]
    fn get_state_sync_records(&self) -> ProviderResult<Vec<WalletStateSyncRecord>> {
        self.0.get_state_sync_records()
    }

    #[inline(always)]
    fn get_state_sync_record_peer_ids(&self) -> ProviderResult<Vec<PeerID>> {
        self.0.get_state_sync_record_peer_ids()
    }

    #[inline(always)]
    fn get_state_sync_record_by_peer_id(
        &self,
        peer_id: PeerID,
    ) -> ProviderResult<Option<WalletStateSyncRecord>> {
        self.0.get_state_sync_record_by_peer_id(peer_id)
    }

    #[inline(always)]
    fn get_state_sync_records_count(&self) -> ProviderResult<usize> {
        self.0.get_state_sync_records_count()
    }

    #[inline(always)]
    fn get_minimum_superset(
        &self,
        min_required_criterion: u64,
    ) -> ProviderResult<(bool, HashSet<(u64, Bytes)>)> {
        self.0.get_minimum_superset(min_required_criterion)
    }
}

impl<DB: Database> WalletStateSyncWriter for BotanixDatabaseProviderRW<DB> {
    fn create_new_state_sync_record(
        &self,
        uuid: UuidID,
        peer_id: PeerID,
        chunks_count: u64,
        data: Option<Vec<(u64, Bytes)>>,
    ) -> ProviderResult<PeerID> {
        let wallet_state_sync_record =
            WalletStateSyncRecord::new(peer_id, uuid, chunks_count, data);
        self.tx.put::<WalletStateSyncs>(peer_id, wallet_state_sync_record)?;
        Ok(peer_id)
    }

    fn append_data_to_state_sync_record(
        &self,
        peer_id: PeerID,
        data: Vec<(u64, Bytes)>,
    ) -> ProviderResult<()> {
        let wallet_state_sync_record = self
            .tx
            .cursor_write::<WalletStateSyncs>()?
            .seek_exact(peer_id)?
            .map(|(_, record)| record);

        if let Some(mut wallet_state_sync_record) = wallet_state_sync_record {
            for (block, data_chunk) in data {
                wallet_state_sync_record.append_data_with_block(data_chunk, block);
            }
            self.tx.put::<WalletStateSyncs>(peer_id, wallet_state_sync_record)?;
        }
        Ok(())
    }

    fn remove_state_sync_record_per_peer_id(&self, peer_id: PeerID) -> ProviderResult<()> {
        self.remove::<WalletStateSyncs>(peer_id..=peer_id)?;
        Ok(())
    }

    fn remove_all_state_sync_records(&self) -> ProviderResult<()> {
        let state_sync_records = self.get_state_sync_record_peer_ids()?;
        if state_sync_records.is_empty() {
            return Ok(());
        }
        for state_sync_record in state_sync_records {
            self.remove::<WalletStateSyncs>(state_sync_record..=state_sync_record)?;
        }
        Ok(())
    }
}
