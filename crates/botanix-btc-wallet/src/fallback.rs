#[cfg(test)]
use crate::fallback::tests::MockableRpcClient;
use crate::{
    bitcoind::BitcoindClient,
    error::{BitcoindAdapterError, BitcoindAdapterResult},
};
use async_trait::async_trait;
use bitcoincore_rpc::json::{EstimateSmartFeeResult, GetBlockResult};
use std::sync::Arc;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum ClientSelection {
    #[default]
    Fallback, // Use all clients with fallback
    Secondary, // Use only seconadary provider only
    Primary,   // Use only first provider
}

#[derive(Clone)]
pub struct FallbackBitcoindClient {
    clients: Vec<BitcoindClientWrapper>,
    selection: ClientSelection,
}

#[derive(Clone)]
pub enum BitcoindClientWrapper {
    Provider1(Arc<BitcoindClient>),
    Provider2(Arc<BitcoindClient>),
    #[cfg(test)]
    Mock(Arc<dyn MockableRpcClient>),
}

impl FallbackBitcoindClient {
    pub fn new(clients: Vec<BitcoindClientWrapper>, selection: ClientSelection) -> Self {
        FallbackBitcoindClient { clients, selection }
    }

    fn filter_clients_to_use(&self) -> Vec<BitcoindClientWrapper> {
        match self.selection {
            ClientSelection::Primary => self.clients.iter().take(1).cloned().collect(),
            ClientSelection::Secondary => self
                .clients
                .iter()
                .filter(|c| matches!(c, BitcoindClientWrapper::Provider2(_)))
                .cloned()
                .collect(),
            ClientSelection::Fallback => self.clients.clone(),
        }
    }

    async fn execute_with_fallback_async<T, F, Fut>(&self, operation: F) -> BitcoindAdapterResult<T>
    where
        F: Fn(BitcoindClientWrapper) -> Fut,
        Fut: std::future::Future<Output = BitcoindAdapterResult<T>> + Send,
        T: Send,
    {
        let mut last_error = None;

        let clients_to_use = match self.filter_clients_to_use() {
            c if c.is_empty() => {
                return Err(BitcoindAdapterError::NoClientsAvailable);
            }
            c => c,
        };

        for (index, client) in clients_to_use.iter().enumerate() {
            let client_clone = client.clone();
            match operation(client_clone).await {
                Ok(result) => {
                    if index > 0 {
                        tracing::warn!("Fallback succeeded with client {}", index);
                    }
                    return Ok(result);
                }
                Err(e) => {
                    tracing::warn!("Client {} failed: {:?}", index, e);
                    // Only continue to next client if we should fallback
                    if !Self::should_fallback(&e) {
                        tracing::debug!("Not falling back for error: {:?}", e);
                        return Err(e);
                    }

                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| BitcoindAdapterError::NoClientsAvailable))
    }

    fn execute_with_fallback_sync<T, F>(&self, operation: F) -> BitcoindAdapterResult<T>
    where
        F: Fn(BitcoindClientWrapper) -> BitcoindAdapterResult<T>,
        T: Send,
    {
        let mut last_error = None;

        let clients_to_use = match self.filter_clients_to_use() {
            c if c.is_empty() => {
                return Err(BitcoindAdapterError::NoClientsAvailable);
            }
            c => c,
        };

        for (index, client) in clients_to_use.iter().enumerate() {
            let client_clone = client.clone();
            match operation(client_clone) {
                Ok(result) => {
                    if index > 0 {
                        tracing::warn!("Fallback succeeded with client {}", index);
                    }
                    return Ok(result);
                }
                Err(e) => {
                    tracing::warn!("Client {} failed: {:?}", index, e);
                    // Only continue to next client if we should fallback
                    if !Self::should_fallback(&e) {
                        tracing::debug!("Not falling back for error: {:?}", e);
                        return Err(e);
                    }

                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| BitcoindAdapterError::NoClientsAvailable))
    }

    fn should_fallback(error: &BitcoindAdapterError) -> bool {
        match error {
            BitcoindAdapterError::BitcoindRpc(_) => true, // Fallback on all rpc errors
            BitcoindAdapterError::NoClientsAvailable => false, /* No point in falling back as no
                                                            * clients are available */
            _ => true, // Fallback on other errors
        }
    }
}

impl FallbackBitcoindClient {
    pub async fn is_synced(&self) -> BitcoindAdapterResult<bool> {
        self.execute_with_fallback_async(|client| async move {
            match client {
                BitcoindClientWrapper::Provider1(rpc) => rpc
                    .get_rpc_client_dyn()
                    .is_synced()
                    .await
                    .map_err(BitcoindAdapterError::BitcoindRpc),
                BitcoindClientWrapper::Provider2(rpc) => rpc
                    .get_rpc_client_dyn()
                    .is_synced()
                    .await
                    .map_err(BitcoindAdapterError::BitcoindRpc),
                #[cfg(test)]
                BitcoindClientWrapper::Mock(mock) => mock.is_synced().await,
            }
        })
        .await
    }

    pub async fn wait_until_synced(&self) -> BitcoindAdapterResult<()> {
        self.execute_with_fallback_async(|client| async move {
            match client {
                BitcoindClientWrapper::Provider1(rpc) => {
                    Ok(rpc.get_rpc_client_dyn().wait_until_synced().await)
                }
                BitcoindClientWrapper::Provider2(rpc) => {
                    Ok(rpc.get_rpc_client_dyn().wait_until_synced().await)
                }
                #[cfg(test)]
                BitcoindClientWrapper::Mock(mock) => Ok(mock.wait_until_synced().await),
            }
        })
        .await
    }

    pub fn get_best_block_hash_rpc(&self) -> BitcoindAdapterResult<bitcoin::BlockHash> {
        self.execute_with_fallback_sync(|client| match client {
            BitcoindClientWrapper::Provider1(rpc) => rpc
                .get_rpc_client_dyn()
                .get_best_block_hash_rpc()
                .map_err(BitcoindAdapterError::BitcoindRpc),
            BitcoindClientWrapper::Provider2(rpc) => rpc
                .get_rpc_client_dyn()
                .get_best_block_hash_rpc()
                .map_err(BitcoindAdapterError::BitcoindRpc),
            #[cfg(test)]
            BitcoindClientWrapper::Mock(mock) => mock.get_best_block_hash_rpc(),
        })
    }

    pub fn get_block_header_rpc(
        &self,
        h: &bitcoin::BlockHash,
    ) -> BitcoindAdapterResult<bitcoin::blockdata::block::Header> {
        self.execute_with_fallback_sync(|client| match client {
            BitcoindClientWrapper::Provider1(rpc) => rpc
                .get_rpc_client_dyn()
                .get_block_header_rpc(h)
                .map_err(BitcoindAdapterError::BitcoindRpc),
            BitcoindClientWrapper::Provider2(rpc) => rpc
                .get_rpc_client_dyn()
                .get_block_header_rpc(h)
                .map_err(BitcoindAdapterError::BitcoindRpc),
            #[cfg(test)]
            BitcoindClientWrapper::Mock(mock) => mock.get_block_header_rpc(h),
        })
    }

    pub fn get_block_hash_rpc(&self, height: u64) -> BitcoindAdapterResult<bitcoin::BlockHash> {
        self.execute_with_fallback_sync(|client| match client {
            BitcoindClientWrapper::Provider1(rpc) => rpc
                .get_rpc_client_dyn()
                .get_block_hash_rpc(height)
                .map_err(BitcoindAdapterError::BitcoindRpc),
            BitcoindClientWrapper::Provider2(rpc) => rpc
                .get_rpc_client_dyn()
                .get_block_hash_rpc(height)
                .map_err(BitcoindAdapterError::BitcoindRpc),
            #[cfg(test)]
            BitcoindClientWrapper::Mock(mock) => mock.get_block_hash_rpc(height),
        })
    }

    pub fn get_txids_rpc(
        &self,
        h: &bitcoin::BlockHash,
    ) -> BitcoindAdapterResult<Vec<bitcoin::Txid>> {
        self.execute_with_fallback_sync(|client| match client {
            BitcoindClientWrapper::Provider1(rpc) => {
                rpc.get_rpc_client_dyn().get_txids_rpc(h).map_err(BitcoindAdapterError::BitcoindRpc)
            }
            BitcoindClientWrapper::Provider2(rpc) => {
                rpc.get_rpc_client_dyn().get_txids_rpc(h).map_err(BitcoindAdapterError::BitcoindRpc)
            }
            #[cfg(test)]
            BitcoindClientWrapper::Mock(mock) => mock.get_txids_rpc(h),
        })
    }

    pub fn get_estimate_smart_fee_rpc(&self) -> BitcoindAdapterResult<EstimateSmartFeeResult> {
        self.execute_with_fallback_sync(|client| match client {
            BitcoindClientWrapper::Provider1(rpc) => rpc
                .get_rpc_client_dyn()
                .get_estimate_smart_fee_rpc()
                .map_err(BitcoindAdapterError::BitcoindRpc),
            BitcoindClientWrapper::Provider2(rpc) => rpc
                .get_rpc_client_dyn()
                .get_estimate_smart_fee_rpc()
                .map_err(BitcoindAdapterError::BitcoindRpc),
            #[cfg(test)]
            BitcoindClientWrapper::Mock(mock) => mock.get_estimate_smart_fee_rpc(),
        })
    }

    pub fn get_block_info_rpc(
        &self,
        block_hash: &bitcoin::BlockHash,
    ) -> BitcoindAdapterResult<GetBlockResult> {
        self.execute_with_fallback_sync(|client| match client {
            BitcoindClientWrapper::Provider1(rpc) => rpc
                .get_rpc_client_dyn()
                .get_block_info_rpc(block_hash)
                .map_err(BitcoindAdapterError::BitcoindRpc),
            BitcoindClientWrapper::Provider2(rpc) => rpc
                .get_rpc_client_dyn()
                .get_block_info_rpc(block_hash)
                .map_err(BitcoindAdapterError::BitcoindRpc),
            #[cfg(test)]
            BitcoindClientWrapper::Mock(mock) => mock.get_block_info_rpc(block_hash),
        })
    }

    pub fn get_block_count_rpc(&self) -> BitcoindAdapterResult<u64> {
        self.execute_with_fallback_sync(|client| match client {
            BitcoindClientWrapper::Provider1(rpc) => rpc
                .get_rpc_client_dyn()
                .get_block_count_rpc()
                .map_err(BitcoindAdapterError::BitcoindRpc),
            BitcoindClientWrapper::Provider2(rpc) => rpc
                .get_rpc_client_dyn()
                .get_block_count_rpc()
                .map_err(BitcoindAdapterError::BitcoindRpc),
            #[cfg(test)]
            BitcoindClientWrapper::Mock(mock) => mock.get_block_count_rpc(),
        })
    }

    pub fn primary(&self) -> Option<Arc<BitcoindClient>> {
        self.clients.iter().find_map(|c| {
            if let BitcoindClientWrapper::Provider1(rpc) = c {
                Some(rpc.clone())
            } else {
                None
            }
        })
    }

    pub fn secondary(&self) -> Option<Arc<BitcoindClient>> {
        self.clients.iter().find_map(|c| {
            if let BitcoindClientWrapper::Provider2(rpc) = c {
                Some(rpc.clone())
            } else {
                None
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockall::{mock, predicate::*};

    // Create a mockable trait
    #[async_trait]
    pub trait MockableRpcClient: Send + Sync {
        async fn is_synced(&self) -> BitcoindAdapterResult<bool>;
        async fn wait_until_synced(&self);

        fn get_best_block_hash_rpc(&self) -> BitcoindAdapterResult<bitcoin::BlockHash>;
        fn get_block_header_rpc(
            &self,
            h: &bitcoin::BlockHash,
        ) -> BitcoindAdapterResult<bitcoin::blockdata::block::Header>;
        fn get_block_hash_rpc(&self, height: u64) -> BitcoindAdapterResult<bitcoin::BlockHash>;
        fn get_txids_rpc(
            &self,
            h: &bitcoin::BlockHash,
        ) -> BitcoindAdapterResult<Vec<bitcoin::Txid>>;
        fn get_estimate_smart_fee_rpc(&self) -> BitcoindAdapterResult<EstimateSmartFeeResult>;
        fn get_block_info_rpc(
            &self,
            block_hash: &bitcoin::BlockHash,
        ) -> BitcoindAdapterResult<GetBlockResult>;
        fn get_block_count_rpc(&self) -> BitcoindAdapterResult<u64>;
    }

    mock! {
        RpcClient {}

        #[async_trait]
        impl MockableRpcClient for RpcClient {
            async fn is_synced(&self) -> BitcoindAdapterResult<bool>;
            async fn wait_until_synced(&self);
            fn get_best_block_hash_rpc(&self) -> BitcoindAdapterResult<bitcoin::BlockHash>;
            fn get_block_header_rpc(&self, h: &bitcoin::BlockHash,) -> BitcoindAdapterResult<bitcoin::blockdata::block::Header>;
            fn get_block_hash_rpc(&self, height: u64) -> BitcoindAdapterResult<bitcoin::BlockHash>;
            fn get_txids_rpc(&self, h: &bitcoin::BlockHash,) -> BitcoindAdapterResult<Vec<bitcoin::Txid>>;
            fn get_estimate_smart_fee_rpc(&self) -> BitcoindAdapterResult<EstimateSmartFeeResult>;
            fn get_block_info_rpc(&self, block_hash: &bitcoin::BlockHash,) -> BitcoindAdapterResult<GetBlockResult>;
            fn get_block_count_rpc(&self) -> BitcoindAdapterResult<u64>;
        }
    }

    //     #[tokio::test]
    //     async fn test_fallback_on_first_client_failure() {
    //         let mut mock_client1 = MockRpcClient::new();
    //         let mut mock_client2 = MockRpcClient::new();

    //         let expected_hash = Hash::new_unique();

    //         // First client fails
    //         mock_client1
    //             .expect_get_latest_blockhash()
    //             .times(1)
    //             .returning(|| Err(SolanaAdapterError::SessionKeyExpired));

    //         // Second client succeeds
    //         mock_client2
    //             .expect_get_latest_blockhash()
    //             .times(1)
    //             .returning(move || Ok(expected_hash));

    //         let clients = vec![
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client1)),
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client2)),
    //         ];

    //         let fallback_client =
    //             FallbackBitcoindClient::new(clients, ClientSelection::Fallback);

    //         // Test
    //         let result = fallback_client.get_latest_blockhash().await;

    //         // Assert
    //         assert!(result.is_ok());
    //         assert_eq!(result.unwrap(), expected_hash);
    //     }

    //     #[tokio::test]
    //     async fn test_no_fallback_on_business_logic_error() {
    //         let mut mock_client1 = MockRpcClient::new();
    //         let mut mock_client2 = MockRpcClient::new();

    //         // First client returns business logic error (shouldn't fallback)
    //         mock_client1
    //             .expect_get_account_data()
    //             .times(1)
    //             .returning(|_| {
    //                 Err(SolanaAdapterError::Redis(redis::RedisError::from((
    //                     redis::ErrorKind::ParseError,
    //                     "test error",
    //                 ))))
    //             });

    //         // Second client should never be called
    //         mock_client2.expect_get_account_data().times(0);

    //         let clients = vec![
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client1)),
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client2)),
    //         ];

    //         let fallback_client =
    //             FallbackBitcoindClient::new(clients, ClientSelection::Fallback);

    //         let test_pubkey = Pubkey::new_unique();
    //         let result = fallback_client.get_account_data(&test_pubkey).await;

    //         assert!(result.is_err());

    //         match result.unwrap_err() {
    //             SolanaAdapterError::Redis(_) => {
    //                 // Expected error type
    //             }
    //             other => {
    //                 panic!("Expected Redis error, got: {:?}", other);
    //             }
    //         }
    //     }

    //     #[tokio::test]
    //     async fn test_all_clients_fail() {
    //         let mut mock_client1 = MockRpcClient::new();
    //         let mut mock_client2 = MockRpcClient::new();

    //         // Both clients fail
    //         mock_client1
    //             .expect_get_latest_blockhash()
    //             .times(1)
    //             .returning(|| Err(SolanaAdapterError::SessionKeyExpired));

    //         mock_client2
    //             .expect_get_latest_blockhash()
    //             .times(1)
    //             .returning(|| Err(SolanaAdapterError::InvalidSessionKey));

    //         let clients = vec![
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client1)),
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client2)),
    //         ];

    //         let fallback_client =
    //             FallbackBitcoindClient::new(clients, ClientSelection::Fallback);

    //         let result = fallback_client.get_latest_blockhash().await;

    //         assert!(result.is_err());
    //         assert!(matches!(
    //             result.unwrap_err(),
    //             SolanaAdapterError::InvalidSessionKey
    //         ));
    //     }

    //     #[tokio::test]
    //     async fn test_no_fallback_on_helius_only_operation() {
    //         let mut mock_client1 = MockRpcClient::new();
    //         let mut mock_client2 = MockRpcClient::new();

    //         mock_client1
    //             .expect_get_account_data()
    //             .times(1)
    //             .returning(|_| {
    //                 Err(SolanaAdapterError::OperationOnlySupportedByHelius(
    //                     "get_priority_fees".to_string(),
    //                 ))
    //             });

    //         // Second client should never be called
    //         mock_client2.expect_get_account_data().times(0);

    //         let clients = vec![
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client1)),
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client2)),
    //         ];

    //         let fallback_client =
    //             FallbackBitcoindClient::new(clients, ClientSelection::Fallback);

    //         let test_pubkey = Pubkey::new_unique();
    //         let result = fallback_client.get_account_data(&test_pubkey).await;

    //         assert!(result.is_err());
    //         assert!(matches!(
    //             result.unwrap_err(),
    //             SolanaAdapterError::OperationOnlySupportedByHelius(_)
    //         ));
    //     }

    //     #[tokio::test]
    //     async fn test_empty_clients_list() {
    //         let clients: Vec<BitcoindClientWrapper> = vec![];
    //         let fallback_client =
    //             FallbackBitcoindClient::new(clients, ClientSelection::Fallback);

    //         let result = fallback_client.get_latest_blockhash().await;

    //         assert!(result.is_err());
    //         assert!(matches!(
    //             result.unwrap_err(),
    //             SolanaAdapterError::NoClientsAvailable
    //         ));
    //     }

    //     #[tokio::test]
    //     async fn test_client_selection_primary_only_fails() {
    //         let mut mock_client1 = MockRpcClient::new();
    //         let mut mock_client2 = MockRpcClient::new();

    //         // Primary client fails
    //         mock_client1
    //             .expect_get_latest_blockhash()
    //             .times(1)
    //             .returning(|| {
    //                 Err(SolanaAdapterError::SolanaRpc(
    //                     solana_client::client_error::ClientError::from(
    //                         solana_client::client_error::ClientErrorKind::Custom(
    //                             "Connection failed".to_string(),
    //                         ),
    //                     ),
    //                 ))
    //             });

    //         // Second client should never be called in Primary mode
    //         mock_client2.expect_get_latest_blockhash().times(0);

    //         let clients = vec![
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client1)),
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client2)),
    //         ];

    //         let fallback_client =
    //             FallbackBitcoindClient::new(clients, ClientSelection::Primary);

    //         let result = fallback_client.get_latest_blockhash().await;

    //         assert!(result.is_err());
    //         assert!(matches!(
    //             result.unwrap_err(),
    //             SolanaAdapterError::SolanaRpc(_)
    //         ));
    //     }

    //     #[tokio::test]
    //     async fn test_client_selection_primary_only() {
    //         let mut mock_client1 = MockRpcClient::new();
    //         let mut mock_client2 = MockRpcClient::new();

    //         let expected_hash = Hash::new_unique();

    //         // Only first client should be called
    //         mock_client1
    //             .expect_get_latest_blockhash()
    //             .times(1)
    //             .returning(move || Ok(expected_hash));

    //         // Second client should never be called even if available
    //         mock_client2.expect_get_latest_blockhash().times(0);

    //         let clients = vec![
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client1)),
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client2)),
    //         ];

    //         let fallback_client =
    //             FallbackBitcoindClient::new(clients, ClientSelection::Primary);

    //         let result = fallback_client.get_latest_blockhash().await;

    //         assert!(result.is_ok());
    //         assert_eq!(result.unwrap(), expected_hash);
    //     }

    //     #[tokio::test]
    //     async fn test_fallback_chain_with_multiple_failures() {
    //         let mut mock_client1 = MockRpcClient::new();
    //         let mut mock_client2 = MockRpcClient::new();
    //         let mut mock_client3 = MockRpcClient::new();

    //         let expected_hash = Hash::new_unique();

    //         // First two clients fail with fallback-worthy errors
    //         mock_client1
    //             .expect_get_latest_blockhash()
    //             .times(1)
    //             .returning(|| {
    //                 Err(SolanaAdapterError::SolanaRpc(
    //                     solana_client::client_error::ClientError::from(
    //                         solana_client::client_error::ClientErrorKind::Custom(
    //                             "Timeout".to_string(),
    //                         ),
    //                     ),
    //                 ))
    //             });

    //         mock_client2
    //             .expect_get_latest_blockhash()
    //             .times(1)
    //             .returning(|| {
    //                 Err(SolanaAdapterError::SolanaRpc(
    //                     solana_client::client_error::ClientError::from(
    //                         solana_client::client_error::ClientErrorKind::Custom(
    //                             "Rate limited".to_string(),
    //                         ),
    //                     ),
    //                 ))
    //             });

    //         // Third client succeeds
    //         mock_client3
    //             .expect_get_latest_blockhash()
    //             .times(1)
    //             .returning(move || Ok(expected_hash));

    //         let clients = vec![
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client1)),
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client2)),
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client3)),
    //         ];

    //         let fallback_client =
    //             FallbackBitcoindClient::new(clients, ClientSelection::Fallback);

    //         let result = fallback_client.get_latest_blockhash().await;

    //         assert!(result.is_ok());
    //         assert_eq!(result.unwrap(), expected_hash);
    //     }

    //     #[tokio::test]
    //     async fn test_first_client_succeeds_no_fallback_needed() {
    //         let mut mock_client1 = MockRpcClient::new();
    //         let mut mock_client2 = MockRpcClient::new();

    //         let expected_hash = Hash::new_unique();

    //         // First client succeeds
    //         mock_client1
    //             .expect_get_latest_blockhash()
    //             .times(1)
    //             .returning(move || Ok(expected_hash));

    //         // Second client should never be called
    //         mock_client2.expect_get_latest_blockhash().times(0);

    //         let clients = vec![
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client1)),
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client2)),
    //         ];

    //         let fallback_client =
    //             FallbackBitcoindClient::new(clients, ClientSelection::Fallback);

    //         let result = fallback_client.get_latest_blockhash().await;

    //         assert!(result.is_ok());
    //         assert_eq!(result.unwrap(), expected_hash);
    //     }

    //     #[tokio::test]
    //     async fn test_all_clients_fail_with_fallback_errors() {
    //         let mut mock_client1 = MockRpcClient::new();
    //         let mut mock_client2 = MockRpcClient::new();

    //         // Both clients fail with errors that should trigger fallback
    //         mock_client1
    //             .expect_get_latest_blockhash()
    //             .times(1)
    //             .returning(|| {
    //                 Err(SolanaAdapterError::SolanaRpc(
    //                     solana_client::client_error::ClientError::from(
    //                         solana_client::client_error::ClientErrorKind::Custom(
    //                             "Connection timeout".to_string(),
    //                         ),
    //                     ),
    //                 ))
    //             });

    //         mock_client2
    //             .expect_get_latest_blockhash()
    //             .times(1)
    //             .returning(|| {
    //                 Err(SolanaAdapterError::SolanaRpc(
    //                     solana_client::client_error::ClientError::from(
    //                         solana_client::client_error::ClientErrorKind::Custom(
    //                             "Service unavailable".to_string(),
    //                         ),
    //                     ),
    //                 ))
    //             });

    //         let clients = vec![
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client1)),
    //             BitcoindClientWrapper::Mock(Arc::new(mock_client2)),
    //         ];

    //         let fallback_client =
    //             FallbackBitcoindClient::new(clients, ClientSelection::Fallback);

    //         let result = fallback_client.get_latest_blockhash().await;

    //         assert!(result.is_err());

    //         let received_error = result.unwrap_err();
    //         match received_error {
    //             SolanaAdapterError::SolanaRpc(err) => match err.kind() {
    //                 solana_client::client_error::ClientErrorKind::Custom(msg) => {
    //                     assert_eq!(msg, "Service unavailable");
    //                 }
    //                 _ => panic!("Expected Custom error, got {:?}", err),
    //             },
    //             _ => panic!("Expected SolanaRpc error, got {:?}", received_error),
    //         }
}
