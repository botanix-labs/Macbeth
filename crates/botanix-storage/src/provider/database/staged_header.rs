use crate::{
    models::HeaderWithPegs,
    provider::{database::provider::BotanixDatabaseProvider, StagedHeader},
    tables,
};
use reth_db_api::{
    cursor::DbCursorRO,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::B256;
use reth_storage_errors::provider::{ProviderError, ProviderResult};

impl<TX: DbTxMut + DbTx> StagedHeader for BotanixDatabaseProvider<TX> {
    fn insert_staged_header(&self, id: B256, header: HeaderWithPegs) -> ProviderResult<()> {
        Ok(self.tx.put::<tables::StagedHeader>(id, header)?)
    }

    fn remove_staged_header(&self, id: B256) -> ProviderResult<bool> {
        let res = self.remove::<tables::StagedHeader>(id..=id)?;
        match res {
            0 => Ok(false),
            1 => Ok(true),
            _ => unreachable!(),
        }
    }

    fn get_staged_headers(&self) -> ProviderResult<Vec<(B256, HeaderWithPegs)>> {
        self.tx
            .cursor_read::<tables::StagedHeader>()?
            .walk(None)?
            .collect::<Result<Vec<(B256, HeaderWithPegs)>, _>>()
            .map_err(ProviderError::Database)
    }
}
