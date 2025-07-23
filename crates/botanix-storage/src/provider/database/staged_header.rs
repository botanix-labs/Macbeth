use crate::{
    models::HeaderWithPegs, provider::database::provider::BotanixDatabaseProvider,
    tables::StagedHeader, BotanixDatabaseProviderRW, StagedHeaderReader, StagedHeaderWriter,
};
use reth_db_api::{
    cursor::DbCursorRO,
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_primitives::B256;
use reth_storage_errors::provider::{ProviderError, ProviderResult};

impl<TX: DbTx> StagedHeaderReader for BotanixDatabaseProvider<TX> {
    fn get_staged_headers(&self) -> ProviderResult<Vec<(B256, HeaderWithPegs)>> {
        self.tx
            .cursor_read::<StagedHeader>()?
            .walk(None)?
            .collect::<Result<Vec<(B256, HeaderWithPegs)>, _>>()
            .map_err(ProviderError::Database)
    }
}

impl<DB: Database> StagedHeaderReader for BotanixDatabaseProviderRW<DB> {
    #[inline(always)]
    fn get_staged_headers(&self) -> ProviderResult<Vec<(B256, HeaderWithPegs)>> {
        self.0.get_staged_headers()
    }
}

impl<DB: Database> StagedHeaderWriter for BotanixDatabaseProviderRW<DB> {
    fn insert_staged_header(&self, id: B256, header: HeaderWithPegs) -> ProviderResult<()> {
        Ok(self.tx.put::<StagedHeader>(id, header)?)
    }

    fn remove_staged_header(&self, id: B256) -> ProviderResult<bool> {
        let res = self.remove::<StagedHeader>(id..=id)?;
        match res {
            0 => Ok(false),
            1 => Ok(true),
            _ => unreachable!(),
        }
    }
}
