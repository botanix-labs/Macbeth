//! Helper macros

/// Creates the block executor type based on the configured feature.
///
/// Note(mattsse): This is incredibly horrible and will be replaced
/// Note(armins) not used
// #[cfg(not(feature = "optimism"))]
// macro_rules! block_executor {
//     ($chain_spec:expr) => {
//         // Botanix change: we construct the noop executor provider else where and do not rely on
//         // these macros reth_node_ethereum::EthExecutorProvider::noop()
//         create_noop_executor_provider($chain_spec)
//     };
// }

#[cfg(feature = "optimism")]
macro_rules! block_executor {
    ($chain_spec:expr) => {
        reth_node_optimism::OpExecutorProvider::optimism($chain_spec)
    };
}
