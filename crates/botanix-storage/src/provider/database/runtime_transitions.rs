use crate::{models::RuntimeVersion, tables, BotanixDatabaseProvider, RuntimeTransitionsReadWrite};
use reth_db_api::{
    cursor::DbCursorRO,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::BlockNumber;
use reth_storage_errors::provider::{ProviderError, ProviderResult};

/// The key where we keep the highest known runtime version. We have roughly ~3
/// trillion years time before this becomes a problem.
const LATEST_RUNTIME_KEY: BlockNumber = u64::MAX;

impl<TX: DbTxMut + DbTx> RuntimeTransitionsReadWrite for BotanixDatabaseProvider<TX> {
    fn insert_runtime_upgrade_version(
        &self,
        height: BlockNumber,
        version: RuntimeVersion,
    ) -> ProviderResult<bool> {
        let latest = self.tx.get::<tables::RuntimeTransitions>(LATEST_RUNTIME_KEY)?;

        // Only record the highest seen runtime versions.
        if let Some(latest) = latest {
            if latest >= version {
                return Ok(false)
            }
        };

        // Insert runtime version transition.
        self.tx.put::<tables::RuntimeTransitions>(LATEST_RUNTIME_KEY, version)?;
        self.tx.put::<tables::RuntimeTransitions>(height, version)?;

        Ok(true)
    }

    fn get_runtime_versions(&self) -> ProviderResult<Vec<(BlockNumber, RuntimeVersion)>> {
        self.tx
            .cursor_read::<tables::RuntimeTransitions>()?
            .walk(None)?
            .filter(|e| {
                let Ok((key, _)) = e else {
                    return true;
                };

                key != &LATEST_RUNTIME_KEY
            })
            .collect::<Result<Vec<(BlockNumber, RuntimeVersion)>, _>>()
            .map_err(ProviderError::Database)
    }

    fn get_last_runtime_version(&self) -> ProviderResult<Option<RuntimeVersion>> {
        self.tx
            .get::<tables::RuntimeTransitions>(LATEST_RUNTIME_KEY)
            .map_err(ProviderError::Database)
    }
}
