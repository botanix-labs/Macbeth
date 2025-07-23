use crate::provider::BotanixProviderFactory;
use reth_db::{
    test_utils::{create_test_rw_db, TempDatabase},
    DatabaseEnv,
};
use std::sync::Arc;

/// Creates test database and provider factory.
///
/// This function creates a temporary in-memory database with a provider factory
/// for testing purposes. The database is automatically cleaned up when the
/// returned factory is dropped, making it ideal for unit tests.
///
/// # Returns
///
/// A `BotanixProviderFactory` instance backed by a temporary database that:
/// - Supports both read and write operations
/// - Is isolated from other tests
/// - Is automatically cleaned up after use
/// - Uses the same database structure as production
///
/// # Usage
///
/// ```rust,ignore
/// use botanix_storage::test_utils::create_test_provider_factory;
///
/// #[test]
/// fn test_snapshot_creation() {
///     let factory = create_test_provider_factory();
///     let provider = factory.provider_rw().unwrap();
///     
///     let snapshot_id = provider.create_new_snapshot(100, block_hash).unwrap();
///     provider.commit().unwrap();
///     
///     assert!(snapshot_id > 0);
/// }
/// ```
///
/// # Thread Safety
///
/// Each call to this function creates a separate database instance, so
/// multiple tests can run concurrently without interfering with each other.
pub fn create_test_provider_factory() -> BotanixProviderFactory<Arc<TempDatabase<DatabaseEnv>>> {
    let db = create_test_rw_db();
    BotanixProviderFactory::new(db)
}
