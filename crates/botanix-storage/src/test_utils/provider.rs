use crate::provider::BotanixProviderFactory;
use reth_db::{
    test_utils::{create_test_rw_db, TempDatabase},
    DatabaseEnv,
};
use std::sync::Arc;

/// Creates test provider factory with mainnet chain spec.
pub fn create_test_provider_factory() -> BotanixProviderFactory<Arc<TempDatabase<DatabaseEnv>>> {
    let db = create_test_rw_db();
    BotanixProviderFactory::new(db)
}
