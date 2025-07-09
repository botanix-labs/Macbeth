use crate::provider::database::provider::{
    BotanixDatabaseProvider, BotanixDatabaseProviderRO, BotanixDatabaseProviderRW,
};
use reth_db::{init_db, mdbx::DatabaseArguments, DatabaseEnv};
use reth_db_api::database::Database;
use reth_storage_errors::provider::ProviderResult;
use std::{path::Path, sync::Arc};

/// A common provider that fetches data from a database or static file.
///
/// This provider implements most provider or provider factory traits.
#[derive(Debug, Clone)]
pub struct BotanixProviderFactory<DB> {
    /// Database
    db: Arc<DB>,
}

impl<DB> BotanixProviderFactory<DB> {
    /// Create new database provider factory.
    pub fn new(db: DB) -> Self {
        Self { db: Arc::new(db) }
    }

    /// Returns reference to the underlying database.
    pub fn db_ref(&self) -> &DB {
        self.db.as_ref()
    }

    #[cfg(any(test, feature = "test-utils"))]
    /// Consumes Self and returns DB
    pub fn into_db(self) -> Arc<DB> {
        self.db
    }
}

impl BotanixProviderFactory<DatabaseEnv> {
    /// Create new database provider by passing a path. [`BotanixProviderFactory`] will own the
    /// database instance.
    pub fn new_with_database_path<P: AsRef<Path>>(
        path: P,
        args: DatabaseArguments,
    ) -> eyre::Result<Self> {
        Ok(Self { db: Arc::new(init_db(path, args)?) })
    }
}

impl<DB: Database> BotanixProviderFactory<DB> {
    /// Returns a provider with a created `DbTx` inside, which allows fetching data from the
    /// database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`]. This may fail if the inner read database transaction fails to open.
    ///
    /// This sets the [`PruneModes`] to [`None`], because they should only be relevant for writing
    /// data.
    #[track_caller]
    pub fn provider(&self) -> ProviderResult<BotanixDatabaseProviderRO<DB>> {
        Ok(BotanixDatabaseProvider::new(self.db.tx()?))
    }

    /// Returns a provider with a created `DbTxMut` inside, which allows fetching and updating
    /// data from the database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`].  This may fail if the inner read/write database transaction fails to
    /// open.
    #[track_caller]
    pub fn provider_rw(&self) -> ProviderResult<BotanixDatabaseProviderRW<DB>> {
        Ok(BotanixDatabaseProviderRW(BotanixDatabaseProvider::new_rw(self.db.tx_mut()?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::SnapshotReader;
    use reth_db::mdbx::DatabaseArguments;

    #[test]
    fn test_provider_factory_with_database_path() {
        let factory = BotanixProviderFactory::new_with_database_path(
            tempfile::TempDir::new().expect("can't create temp directory").into_path(),
            DatabaseArguments::new(Default::default()),
        )
        .unwrap();

        let provider = factory.provider().unwrap();
        provider.get_first_chunk_id().unwrap();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.get_first_chunk_id().unwrap();
    }
}
