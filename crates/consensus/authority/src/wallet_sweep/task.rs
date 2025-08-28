use crate::{signing::SigningStateMachine, wallet_sweep::timer::SigningHandlerTimer, Storage};
use botanix_authority_metrics::AuthorityMetrics;
use botanix_authority_rsp::RandomSource;
use botanix_storage::{
    models::{WalletSweepSession, WalletSweepSessionId},
    WalletSweepSessionReader, WalletSweepSessionWriter,
};
use botanix_wallet_sweep::create_psbt_async;
use btc_server_client::{BtcServerExtendedApi, WalletSweepSessionUpdateResponse};
use btcserverlib::signer::{SigningSessionId, SigningSessionType};
use eyre::WrapErr;
use futures::pin_mut;
use futures_util::StreamExt;
use reth_chain_state::CanonStateSubscriptions;
use reth_db::table::Decompress;
use reth_network::frost::manager::ToFrostManager;
use reth_primitives::{alloy_primitives::FixedBytes, keccak256};
use reth_provider::{BlockReaderIdExt, StateProviderFactory};
use std::{ops::Deref, pin::Pin, sync::Arc, time::Duration};
use tokio::time::sleep;
use tracing::{debug, error, info, trace, warn};

const SIGNING_EXPECTED_DURATION: Duration = Duration::from_secs(60);
const ERROR_RETRY_INTERVAL: Duration = Duration::from_secs(10);

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
    ) -> Option<SigningHandlerTimer> {
        let sweep_session_id: WalletSweepSessionId =
            session_update.session_id.as_slice().try_into().expect("todo: handle error");

        trace!(
            %sweep_session_id,
            "Update wallet sweep session from btc-server"
        );

        // Check if we had another session
        match self.storage.botanix_database_factory.get_wallet_sweep_session() {
            Ok(Some((existing_sweep_session_id, _))) => {
                if existing_sweep_session_id == sweep_session_id {
                    trace!(
                        %sweep_session_id,
                        "Wallet sweep session already exists. Skip update"
                    );

                    return None;
                } else {
                    debug!(
                        new_sweep_session_id = %sweep_session_id,
                        %existing_sweep_session_id,
                        "Another wallet sweep session already exist. It will be replaced with the new one"
                    );
                }
            }
            Err(e) => {
                error!("failed to fetch current wallet sweep session from database: {e}");

                return Some(SigningHandlerTimer::after(ERROR_RETRY_INTERVAL));
            }
            Ok(None) => {}
        };

        let session = match WalletSweepSession::decompress(session_update.session_bytes) {
            Ok(session) => session,
            Err(e) => {
                error!("Failed to deserialize wallet sweep session: {e}");

                return Some(SigningHandlerTimer::after(ERROR_RETRY_INTERVAL));
            }
        };

        self.storage
            .botanix_database_factory
            .update_wallet_sweep_session(session.clone())
            .expect("failed to update wallet sweep session");

        Some(SigningHandlerTimer::after(SIGNING_EXPECTED_DURATION))
    }

    async fn initiate_signing_session(
        &mut self,
        sweep_session_id: WalletSweepSessionId,
        sweep_session: WalletSweepSession,
    ) -> Option<SigningSessionId> {
        let signing_session_id = generate_signing_session_id(sweep_session_id);

        info!(
            %sweep_session_id,
            %signing_session_id,
            "Initiating FROST signing for sweep PSBT"
        );

        // Create the sweep PSBT
        let sweep_psbt = match create_psbt_async(sweep_session, &mut self.btc_server).await {
            Ok(psbt) => {
                trace!(
                    %sweep_session_id,
                    %signing_session_id,
                    "Successfully created sweep PSBT with {} inputs, {} outputs",
                    psbt.inputs.len(),
                    psbt.outputs.len()
                );
                psbt
            }
            Err(e) => {
                error!(
                    %sweep_session_id,
                    %signing_session_id,
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
                %sweep_session_id,
                %signing_session_id,
                "Failed to initiate FROST signing for sweep PSBT: {}", e
            );

            None
        } else {
            info!(
                %sweep_session_id,
                %signing_session_id,
                "Initiated FROST signing for sweep PSBT"
            );

            Some(signing_session_id)
        }
    }

    async fn handle_psbt_signing_session(&mut self) -> SigningHandlerTimer {
        let is_coordinator = self.signing_state_machine.is_coordinator();

        let (sweep_session_id, sweep_session) = match self
            .storage
            .botanix_database_factory
            .get_wallet_sweep_session()
        {
            Ok(Some((session_id, session))) => (session_id, session),
            Ok(None) => {
                // If we don't have a wallet sweep session, ensure no signing session is active
                let mut active_signing_session_id = self.signing_session_id.lock().await;

                if let Some(signing_session_id) = *active_signing_session_id {
                    debug!(
                        %signing_session_id,
                        "No wallet sweep session found, but there is an active signing session. Reject it."
                    );

                    self.signing_state_machine
                        .reject_signing_session(signing_session_id)
                        .await
                        .expect("todo: handle error");

                    *active_signing_session_id = None;
                } else {
                    trace!("No wallet sweep session found. Idle for 1 minute");
                }

                return SigningHandlerTimer::pause();
            }
            Err(e) => {
                error!("Failed to fetch wallet sweep session from database: {e}");

                return SigningHandlerTimer::after(ERROR_RETRY_INTERVAL);
            }
        };

        // Check if we have an active signing session
        let mut active_signing_session_id = self.signing_session_id.lock().await;

        if let Some(signing_session_id) = *active_signing_session_id {
            // Check if the corresponding signing session exists and its status
            if let Some(signing_session) =
                self.signing_state_machine.get_signing_session(signing_session_id).await
            {
                if signing_session.state().is_finalized() {
                    // Signing is completed successfully.

                    // Remove the wallet sweep session from database as completed
                    self.storage
                        .botanix_database_factory
                        .clear_wallet_sweep_session()
                        .expect("todo: failed to clear wallet sweep session");

                    *active_signing_session_id = None;

                    info!(
                        %sweep_session_id,
                        %signing_session_id,
                        "Wallet sweep signing session is completed successfully"
                    );

                    return SigningHandlerTimer::pause();
                } else if signing_session.state().has_failed() {
                    warn!(
                        %sweep_session_id,
                        %signing_session_id,
                        "Wallet sweep signing session failed. Retrying"
                    );
                } else if signing_session.state().is_running() {
                    warn!(
                        %sweep_session_id,
                        %signing_session_id,
                        "Signing session still not completed after 1 minute. Reject and retry"
                    );

                    // Reject the incomplete session
                    self.signing_state_machine
                        .reject_signing_session(signing_session_id)
                        .await
                        .expect("todo: handle error");
                } else {
                    warn!(
                        %sweep_session_id,
                        %signing_session_id,
                        "Signing session in unexpected state after 1 minute. Reject and retry"
                    );

                    // Reject the session in unexpected state
                    self.signing_state_machine
                        .reject_signing_session(signing_session_id)
                        .await
                        .expect("todo: handle error");
                }
            } else {
                warn!(
                    %sweep_session_id,
                    %signing_session_id,
                    "Signing session not found after 1 minute for some reason. Retrying"
                );
            }
        } else {
            if is_coordinator {
                trace!(
                    %sweep_session_id,
                    "No active wallet sweep signing session found. Start a new one"
                );
            } else {
                trace!(
                    %sweep_session_id,
                    "No active wallet sweep signing session found. Waiting for a new one"
                );
            }
        }

        // Only coordinator can start signing session
        if !is_coordinator {
            return SigningHandlerTimer::after(SIGNING_EXPECTED_DURATION);
        }

        // We borrow mutable self bellow so we can't keep immutable borrowed lock
        drop(active_signing_session_id);

        // Start signing session again
        let new_signing_session_id =
            self.initiate_signing_session(sweep_session_id, sweep_session).await;

        let mut active_signing_session_id = self.signing_session_id.lock().await;
        *active_signing_session_id = new_signing_session_id;

        SigningHandlerTimer::after(SIGNING_EXPECTED_DURATION)
    }

    async fn abort_wallet_sweep_session(&mut self) -> Option<SigningHandlerTimer> {
        trace!("Wallet sweep session abort is requested from btc-server");

        // Clear the wallet sweep session from database
        let removed_session_id = self
            .storage
            .botanix_database_factory
            .clear_wallet_sweep_session()
            .expect("todo: handle error");

        if let Some(sweep_session_id) = removed_session_id {
            info!(
                %sweep_session_id,
                "Wallet sweep session is aborted"
            );
        } else {
            trace!("No wallet sweep session found to abort");
        }

        Some(SigningHandlerTimer::immediately())
    }

    pub async fn run(mut self) {
        // The timer to run signing session handler
        let mut signing_handler_schedule = SigningHandlerTimer::default();

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
                            let schedule_update = if message.session_id.is_empty() {
                                self.abort_wallet_sweep_session().await
                            } else {
                                self.handle_wallet_sweep_session_update(message).await
                            };

                            if let Some(schedule) = schedule_update {
                                signing_handler_schedule = schedule;
                            };
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
                _ = signing_handler_schedule.wait() => {
                    signing_handler_schedule = self.handle_psbt_signing_session().await;
                }
            }

            // Avoid busy loop
            sleep(Duration::from_secs(1)).await;
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

    SigningSessionId::new_sweep_session(*signing_session_id_payload)
}
