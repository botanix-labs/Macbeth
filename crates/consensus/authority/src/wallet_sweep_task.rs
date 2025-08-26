use crate::{signing::SigningStateMachine, Storage};
use botanix_authority_metrics::AuthorityMetrics;
use botanix_authority_rsp::RandomSource;
use botanix_storage::{
    models::{WalletSweepSession, WalletSweepSessionId},
    WalletSweepSessionReader, WalletSweepSessionWriter,
};
use botanix_wallet_sweep::create_psbt_async;
use btc_server_client::{BtcServerExtendedApi, WalletSweepSessionUpdateResponse};
use btcserverlib::signer::{SigningSessionId, SigningSessionType};
use futures::pin_mut;
use futures_util::StreamExt;
use reth_chain_state::CanonStateSubscriptions;
use reth_db::table::Decompress;
use reth_network::frost::{manager::ToFrostManager, SigningPsbtType};
use reth_primitives::{alloy_primitives::FixedBytes, keccak256};
use reth_provider::{BlockReaderIdExt, StateProviderFactory};
use std::{ops::Deref, sync::Arc};
use tracing::{debug, error, info, trace, warn};

#[derive(Clone)]
pub struct WalletSweepTask<EF, BF, RDB, BDB, ToFrostMan, Source, BtcServerClient> {
    // Frost network Handler
    signing_state_machine: SigningStateMachine<ToFrostMan, Source, BtcServerClient>,
    // Shared storage to insert aggregate public key
    storage: Storage<EF, BF, RDB, BDB>,
    // btc-server client
    btc_server: BtcServerClient,
    // Authority Metrics
    metrics: Arc<AuthorityMetrics>,
    signing_session_id: Arc<tokio::sync::Mutex<Option<SigningSessionId>>>,
}

impl<EF, BF, RDB, BDB, ToFrostMan, Source, BtcServerClient>
    WalletSweepTask<EF, BF, RDB, BDB, ToFrostMan, Source, BtcServerClient>
where
    EF: Clone + 'static + Send + Sync,
    BF: Clone + 'static + Send + Sync,
    RDB: Clone + 'static + Send + Sync,
    BDB: WalletSweepSessionReader + WalletSweepSessionWriter + Clone + 'static + Send + Sync,
    ToFrostMan: Clone + 'static + Send + Sync + ToFrostManager,
    BtcServerClient: BtcServerExtendedApi + Clone,
    Source: RandomSource + Clone + Send + Sync + 'static,
{
    pub fn new(
        signing_state_machine: SigningStateMachine<ToFrostMan, Source, BtcServerClient>,
        storage: Storage<EF, BF, RDB, BDB>,
        btc_server: BtcServerClient,
        metrics: Arc<AuthorityMetrics>,
    ) -> Self {
        Self {
            signing_state_machine,
            storage,
            btc_server,
            metrics,
            signing_session_id: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Check for wallet sweep sessions and create PSBTs directly for FROST signing
    async fn handle_wallet_sweep_session_update(
        &mut self,
        session_update: WalletSweepSessionUpdateResponse,
    ) {
        let session_id: WalletSweepSessionId =
            session_update.session_id.as_slice().try_into().expect("todo: handle error");

        trace!(
            sweep_session_id = hex::encode(&session_id),
            "Update wallet sweep session from btc-server"
        );

        // Check if we had another session
        match self.storage.botanix_database_factory.get_wallet_sweep_session() {
            Ok(Some((existing_session_id, _))) => {
                if existing_session_id == session_id {
                    trace!(
                        sweep_session_id = hex::encode(&session_id),
                        "Wallet sweep session already exists. Skip update"
                    );

                    return;
                } else {
                    debug!(
                        new_sweep_session_id = hex::encode(&session_id),
                        existing_sweep_session_id = hex::encode(&existing_session_id),
                        "Another wallet sweep session already exist. It will be replaced with the new one"
                    );
                }
            }
            Err(e) => {
                error!("failed to fetch current wallet sweep session from database: {e}");

                return;
            }
            Ok(None) => {}
        };

        let session = match WalletSweepSession::decompress(session_update.session_bytes) {
            Ok(session) => session,
            Err(e) => {
                error!("Failed to deserialize wallet sweep session: {e}");

                return;
            }
        };

        self.storage
            .botanix_database_factory
            .update_wallet_sweep_session(session.clone())
            .expect("failed to update wallet sweep session");
    }

    async fn initiate_signing_session(
        &mut self,
        sweep_session_id: WalletSweepSessionId,
        sweep_session: WalletSweepSession,
    ) -> Option<SigningSessionId> {
        let signing_session_id = generate_signing_session_id(sweep_session_id);

        info!(
            sweep_session_id = hex::encode(&sweep_session_id),
            signing_session_id = hex::encode(&signing_session_id),
            "Initiating FROST signing for sweep PSBT"
        );

        // Create the sweep PSBT
        let sweep_psbt = match create_psbt_async(sweep_session, &mut self.btc_server).await {
            Ok(psbt) => {
                trace!(
                    sweep_session_id = hex::encode(&sweep_session_id),
                    signing_session_id = hex::encode(&signing_session_id),
                    "Successfully created sweep PSBT with {} inputs, {} outputs",
                    psbt.inputs.len(),
                    psbt.outputs.len()
                );
                psbt
            }
            Err(e) => {
                error!(
                    sweep_session_id = hex::encode(&sweep_session_id),
                    signing_session_id = hex::encode(&signing_session_id),
                    "Failed to create sweep PSBT: {}",
                    e
                );

                return None;
            }
        };

        // Serialize the PSBT for signing
        let psbt_bytes = sweep_psbt.serialize();

        // Initiate FROST signing-session with the directly created PSBT
        if let Err(e) = self
            .signing_state_machine
            .initiate_signing_session(signing_session_id, psbt_bytes)
            .await
        {
            error!(
                sweep_session_id = hex::encode(&sweep_session_id),
                %signing_session_id,
                "Failed to initiate FROST signing for sweep PSBT: {}", e
            );

            None
        } else {
            info!(
                sweep_session_id = hex::encode(&sweep_session_id),
                signing_session_id = hex::encode(&signing_session_id),
                "Initiated FROST signing for sweep PSBT"
            );

            Some(signing_session_id)
        }
    }

    async fn handle_psbt_signing_session(&mut self) {
        trace!("Handle signing session for wallet sweep");

        let (sweep_session_id, sweep_session) = match self
            .storage
            .botanix_database_factory
            .get_wallet_sweep_session()
        {
            Ok(Some((session_id, session))) => (session_id, session),
            Ok(None) => {
                // If we don't have a wallet sweep session, ensure no signing session is active
                let mut maybe_signing_session_id = self.signing_session_id.lock().await;

                if let Some(signing_session_id) = *maybe_signing_session_id {
                    debug!(
                        signing_session_id = hex::encode(&signing_session_id),
                        "No wallet sweep session found, but there is an active signing session. Reject it."
                    );

                    self.signing_state_machine.remove_signing_session(signing_session_id).await;

                    *maybe_signing_session_id = None;
                } else {
                    trace!("No wallet sweep session found");
                }

                return;
            }
            Err(e) => {
                error!("Failed to fetch wallet sweep session from database: {e}");

                return;
            }
        };

        // Check if we have an active signing session
        let mut maybe_signing_session_id = self.signing_session_id.lock().await;

        if let Some(signing_session_id) = *maybe_signing_session_id {
            // Check if the corresponding signing session exists and its status
            if let Some(signing_session) =
                self.signing_state_machine.get_signing_session(signing_session_id).await
            {
                if signing_session.state().is_finalized() {
                    // Signing completed successfully.
                    info!(
                        sweep_session_id = hex::encode(&sweep_session_id),
                        signing_session_id = hex::encode(&signing_session_id),
                        "Wallet sweep signing session completed successfully"
                    );

                    *maybe_signing_session_id = None;

                    return;
                } else if signing_session.state().has_failed() {
                    warn!(
                        sweep_session_id = hex::encode(&sweep_session_id),
                        signing_session_id = hex::encode(&signing_session_id),
                        "Signing session failed after 1 minute, remove and retry"
                    );

                    // Remove the failed session
                    self.signing_state_machine.remove_signing_session(signing_session_id).await;
                } else if signing_session.state().is_running() {
                    warn!(
                        sweep_session_id = hex::encode(&sweep_session_id),
                        signing_session_id = hex::encode(&signing_session_id),
                        "Signing session still not completed after 1 minute, reject and retry"
                    );

                    // Remove the incomplete session
                    self.signing_state_machine.remove_signing_session(signing_session_id).await;
                } else {
                    warn!(
                            sweep_session_id = hex::encode(&sweep_session_id),
                            signing_session_id = hex::encode(&signing_session_id),
                            "Signing session in unexpected state after 1 minute, removing and will retry on next check"
                        );

                    // Remove the session in unexpected state
                    self.signing_state_machine.remove_signing_session(signing_session_id).await;
                }
            } else {
                warn!(
                    sweep_session_id = hex::encode(&sweep_session_id),
                    signing_session_id = hex::encode(&signing_session_id),
                    "Signing session not found after 1 minute for some reason, retrying"
                );
            }
        } else {
            trace!("No active wallet sweep signing session, starting new one");
        }

        // We borrow mutable self bellow so we can't keep immutable borrowed lock
        drop(maybe_signing_session_id);

        // Start signing session again
        let new_signing_session_id =
            self.initiate_signing_session(sweep_session_id, sweep_session).await;

        let mut maybe_signing_session_id = self.signing_session_id.lock().await;
        *maybe_signing_session_id = new_signing_session_id;
    }

    pub async fn run(mut self) {
        let mut signing_check_interval =
            tokio::time::interval(tokio::time::Duration::from_secs(60));

        loop {
            let request = btc_server_client::WalletSweepSessionUpdatesRequest {};
            let mut stream =
                match self.btc_server.subscribe_to_wallet_sweep_session_updates(request).await {
                    Ok(stream) => {
                        debug!("Connected to wallet sweep session updates");

                        stream
                    }
                    Err(e) => {
                        error!("Failed to subscribe to wallet sweep session updates: {e}");

                        continue;
                    }
                };

            pin_mut!(stream);

            tokio::select! {
                // Handle incoming wallet sweep session update stream messages
                received = stream.next() => {
                    match received {
                        Some(Ok(message)) => {
                            trace!("Received message from wallet sweep session stream");

                            self.handle_wallet_sweep_session_update(message).await;
                        }
                        Some(Err(e)) => {
                            error!("Wallet sweep session update stream error: {}", e);

                            continue;
                        }
                        None => {
                            warn!("Wallet sweep session stream disconnected. Reconnect in 1 second");

                            continue;
                        }
                    }
                }

                // Handle periodic timer ticks (every minute)
                _ = signing_check_interval.tick() => {
                    self.handle_psbt_signing_session().await;
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }
}

/// Hash wallet sweep session id with current time to get unique signing session id
fn generate_signing_session_id(sweep_session_id: WalletSweepSessionId) -> SigningSessionId {
    let signing_session_id_payload = [
        sweep_session_id.as_slice(),
        &std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()
            .to_le_bytes(),
    ]
    .concat();

    let signing_session_id_payload = keccak256(signing_session_id_payload);

    SigningSessionId::new_sweep_session(signing_session_id_payload.as_ref())
}
