use std::time::Duration;

use bitcoin::{
    hashes::{sha256::Hash as Sha256Hash, FromSliceError},
    secp256k1::hashes::Hash,
};
use btcserverlib::extended_client::{BtcServerExtendedClient, GrpcClientError};
use client::{Empty, GetAllUtxosResponse, ResetAllUtxosRequest};
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_network::frost::{
    manager::{FrostCommand, ToFrostManager},
    PeerMessageResponse,
};
use reth_primitives::{extra_data_header::ExtraDataHeaderDeserializeError, header_ext::HeaderExt};
use reth_provider::{BlockReaderIdExt, ExecutorFactory, ProviderError};
use tokio::sync::{mpsc::error::SendError, RwLock};
use tracing::{debug, error, trace, warn};

use crate::{
    compressor::{Compressor, Error as CompressorError, ProstMessageSerdelizer},
    utils::{generate_utxo_merkel_root, UtxoMerkelRootError},
    Storage,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum UtxoSyncError {
    #[error("db provider error: {0}")]
    LatestBlockError(#[from] ProviderError),
    #[error("deserilaize extra data header : {0}")]
    DeserializeExtraDataHeaderError(#[from] ExtraDataHeaderDeserializeError),
    #[error("btc server client error: {0}")]
    BtcServerClientError(#[from] GrpcClientError),
    #[error("frost manager send error: {0}")]
    FrostManagerSendError(#[from] SendError<FrostCommand>),
    #[error("peer never responded with utxo set, timer elapsed")]
    PeerUtxoSetTimeout,
    #[error("Failed to receive a frost message from a peer {0}")]
    FrostRecv(tokio::sync::oneshot::error::RecvError),
    #[error("Failed to decompress utxo set data {0}")]
    CompressorError(#[from] CompressorError),
    #[error("Failed to generate utxo merkel root {0}")]
    UtxoMerkelRootError(#[from] UtxoMerkelRootError),
    #[error("UTXO set from peer is not in sync with the latest block, current utxo set merkel root: {0}, latest utxo set merkel root: {1}")]
    UtxoSetNotInSync(Sha256Hash, Sha256Hash),
    #[error("Failed to convert slide to sha256 hash {0}")]
    Sha256HashError(#[from] FromSliceError),
}

pub(crate) trait UTXOSync {
    async fn sync_utxo_set(&self) -> Result<(), UtxoSyncError>;
}

#[derive(Debug, Clone)]
pub(crate) struct UTXOSyncEngine<EF, BF, DB, ToFrostMan> {
    storage: Storage<EF, BF, DB>,
    btc_server: BtcServerExtendedClient,
    to_frost_manager: ToFrostMan,
    compressor: Compressor,
}

impl<EF, BF, DB, ToFrostMan> UTXOSyncEngine<EF, BF, DB, ToFrostMan>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: ExecutorFactory + Clone + 'static,
    ToFrostMan: ToFrostManager + Clone + 'static,
    DB: BlockReaderIdExt + Clone + 'static,
{
    pub(crate) fn new(
        storage: Storage<EF, BF, DB>,
        btc_server: BtcServerExtendedClient,
        to_frost_manager: ToFrostMan,
        compressor: Compressor,
    ) -> Self {
        Self { storage, btc_server, to_frost_manager, compressor }
    }
}

impl<EF, BF, DB, ToFrostMan> UTXOSync for UTXOSyncEngine<EF, BF, DB, ToFrostMan>
where
    BF: BitcoindFactory + Clone + 'static,
    EF: ExecutorFactory + Clone + 'static,
    ToFrostMan: ToFrostManager + Clone + 'static,
    DB: BlockReaderIdExt + Clone + 'static,
{
    // Note: this function should not be called unless we are fully synced
    async fn sync_utxo_set(&self) -> Result<(), UtxoSyncError> {
        trace!(target: "consensus::authority::UTXOSync::sync_utxo_set", "syncing utxo set");
        let guard = self.storage.read().await;
        let client = guard.client.clone();
        drop(guard);
        let mut btc_server = self.btc_server.clone();

        let latest_header = client.latest_header()?.expect("should get latest block");
        let latest_merkel_root = latest_header.get_utxo_set_merkle_root()?;

        if latest_header.number == 0 {
            debug!(target: "consensus::authority::UTXOSync::sync_utxo_set", "genesis block, no utxo set to sync");
            return Ok(());
        }

        // get utxo set from btc server
        let latest_utxo_commitment = Sha256Hash::from_slice(
            btc_server.get_utxo_merkle_root(Empty {}).await?.merkle_root.as_slice(),
        )?;
        if latest_merkel_root == latest_utxo_commitment {
            debug!(target: "consensus::authority::UTXOSync::sync_utxo_set", "utxo set is in sync");
            // All done! We are in sync
            return Ok(());
        }

        // Since we are not in sync, we need to get the utxo set from a peer
        let (peer_messages_tx, peer_messages_rx) = tokio::sync::oneshot::channel();
        self.to_frost_manager
            .send_command(FrostCommand::GetPeerMessagesStream(peer_messages_tx))?;
        // TODO remove unwrap()
        let mut peer_messages_rx = peer_messages_rx.await.unwrap();

        // Request the utxo set from a peer
        self.to_frost_manager.send_command(FrostCommand::GetUtxoSetFromPeer)?;
        // try getting the utxo set from the random peer we pinged
        match tokio::time::timeout(Duration::from_secs(60), peer_messages_rx.recv()).await {
            Ok(peer_message) => {
                if let Some((_peer_id, peer_message)) = peer_message {
                    if let PeerMessageResponse::Utxo(utxo_set) = peer_message {
                        // process the utxo set
                        debug!(target: "consensus::authority::block_fetcher::sync_utxo_set", "Got utxo set from peer {:?}", utxo_set);
                        let data = utxo_set.data;
                        let decompressed = self.compressor.decompress(&data).await.map_err(|e| {
                            error!(target: "consensus::authority::block_fetcher::sync_utxo_set", "Failed to decompress utxo set data {:?}", e);
                            UtxoSyncError::CompressorError(e)
                        })?;

                        let utxo_set = ProstMessageSerdelizer::<GetAllUtxosResponse>::deserialize(
                            decompressed,
                        )?;

                        let merkel_root = generate_utxo_merkel_root(&utxo_set.utxos)?;
                        if merkel_root != latest_utxo_commitment {
                            return Err(UtxoSyncError::UtxoSetNotInSync(
                                merkel_root,
                                latest_utxo_commitment,
                            ));
                        }

                        // Report to btc server to sync utxo set
                        btc_server
                            .reset_all_utxos(ResetAllUtxosRequest { utxos: utxo_set.utxos })
                            .await?;
                    }
                } else {
                    // TODO better error variant
                    return Err(UtxoSyncError::PeerUtxoSetTimeout);
                }
            }
            Err(e) => {
                warn!(target: "consensus::authority::block_fetcher::sync_utxo_set", ?e, "Failed to get get utxo set rom a peer within 60 secs");
                return Err(UtxoSyncError::PeerUtxoSetTimeout);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::pbft::tests::FrostHandleMock;
    use reth_btc_wallet::{bitcoind::BitcoindConfig, test_utils::MockBitcoindFactory};
    use reth_node_ethereum::EthEvmConfig;
    use reth_primitives::BOTANIX_TESTNET;
    use reth_provider::test_utils::{MockEthProvider, TestExecutorFactory};

    use super::*;

    #[tokio::test]
    async fn create_new_utxo_set_sync_engine() {
        let mock_eth_provider = MockEthProvider::default();
        let sk = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let dummy_pk = secp256k1::PublicKey::from_secret_key_global(&sk);
        let executor_factory = TestExecutorFactory::default();
        let mock_to_frost_man = FrostHandleMock {};

        let storage = Storage::new(
            vec![],
            vec![],
            0,
            dummy_pk.clone(),
            bitcoin::Network::Regtest,
            None,
            vec![],
            EthEvmConfig::default(),
            BOTANIX_TESTNET.clone(),
            MockBitcoindFactory::new(BitcoindConfig::default()),
            executor_factory,
            mock_eth_provider.clone(),
        );
    }
}
