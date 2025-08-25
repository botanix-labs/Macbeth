use crate::{signing::SigningStateMachine, Storage};
use botanix_authority_metrics::AuthorityMetrics;
use botanix_authority_rsp::RandomSource;
use botanix_storage::{
    models::WalletSweepSession, WalletSweepSessionReader, WalletSweepSessionWriter,
};
use botanix_wallet_sweep::create_psbt_async;
use btc_server_client::{BtcServerExtendedApi, WalletSweepSessionUpdateResponse};
use futures::pin_mut;
use futures_util::StreamExt;
use reth_chain_state::CanonStateSubscriptions;
use reth_db::table::Decompress;
use reth_network::frost::{manager::ToFrostManager, SigningPsbtType};
use reth_primitives::alloy_primitives::FixedBytes;
use reth_provider::{BlockReaderIdExt, StateProviderFactory};
use std::sync::Arc;
use tracing::{debug, error, info, trace, warn};

pub struct WalletSweepTask<EF, BF, RDB, BDB, ToFrostMan, Source, BtcServerClient> {
    // Frost network Handler
    signing_state_machine: SigningStateMachine<ToFrostMan, Source, BtcServerClient>,
    // Shared storage to insert aggregate public key
    storage: Storage<EF, BF, RDB, BDB>,
    // btc-server client
    btc_server: BtcServerClient,
    // Authority Metrics
    metrics: Arc<AuthorityMetrics>,
}

impl<EF, BF, RDB, BDB, ToFrostMan, Source, BtcServerClient>
    WalletSweepTask<EF, BF, RDB, BDB, ToFrostMan, Source, BtcServerClient>
where
    EF: 'static + Send + Sync,
    BF: 'static + Send + Sync,
    RDB: 'static + Send + Sync,
    BDB: WalletSweepSessionReader + WalletSweepSessionWriter + Clone + 'static + Send + Sync,
    ToFrostMan: Clone + 'static + Send + Sync + ToFrostManager,
    BtcServerClient: BtcServerExtendedApi + Clone,
    Source: RandomSource + Clone,
{
    pub fn new(
        signing_state_machine: SigningStateMachine<ToFrostMan, Source, BtcServerClient>,
        storage: Storage<EF, BF, RDB, BDB>,
        btc_server: BtcServerClient,
        metrics: Arc<AuthorityMetrics>,
    ) -> Self {
        Self { signing_state_machine, storage, btc_server, metrics }
    }

    /// Check for wallet sweep sessions and create PSBTs directly for FROST signing
    async fn handle_wallet_sweep_session_update(
        &mut self,
        session_update: WalletSweepSessionUpdateResponse,
    ) {
        let session_id =
            session_update.session_id.as_slice().try_into().expect("todo: handle error");

        // skip this update if the session already exists
        if self
            .storage
            .botanix_database_factory
            .is_wallet_sweep_session_exists(session_id)
            .expect("todo: handle error")
        {
            trace!(target: "consensus::authority::frost_task::WalletSweepTask", "Skip wallet sweep session update - session already exists");

            return;
        }

        // check if we another session and reject it first
        match self.storage.botanix_database_factory.get_wallet_sweep_session() {
            Ok(Some((existing_session_id, _))) => {
                warn!(
                    new_session_id = hex::encode(&session_id),
                    existing_session_id = hex::encode(&existing_session_id),
                    "another wallet sweep session already exist. reject it first"
                );

                // TODO: Reject exising session

                // // Check if we already have a signing session for this sweep
                // if self.signing_state_machine.signing_session_exists(signing_session_id).await {
                //     trace!(target:
                // "consensus::authority::frost_task::check_and_initiate_sweep_signing",
                //    "Signing session already exists for sweep: {}",
                // hex::encode(&signing_session_id));     return;
                // }
            }
            Err(e) => {
                error!("failed to fetch current wallet sweep session from database: {e}");

                return;
            }
            _ => {}
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
            .update_wallet_sweep_session(session)
            .expect("failed to update wallet sweep session");

        // The logic bellow is only for the coordinator
        if !self.signing_state_machine.is_coordinator() {
            trace!("Not coordinator, waiting for sweep PSBT from coordinator");

            return;
        }

        // TODO: We need to run it periodically to check if we have a session
        //  and restart it if it fails
        self.initiate_signing_session().await;
    }

    async fn initiate_signing_session(&mut self) {
        let (session_id, sweep_session) =
            match self.storage.botanix_database_factory.get_wallet_sweep_session() {
                Ok(Some((session_id, session))) => (session_id, session),
                Err(e) => {
                    error!("failed to fetch current wallet sweep session from database: {e}");

                    return;
                }
                _ => {
                    trace!("No wallet sweep session found, skipping signing initiation");

                    // Do nothing if the session doesn't exist
                    return;
                }
            };

        // Check if we already have a signing session for this sweep
        if let Some(signing_session) =
            self.signing_state_machine.get_signing_session(*session_id).await
        {
            if signing_session.state().has_failed() {
                // It's failed, so we remove it
                debug!(
                    session_id = hex::encode(&session_id),
                    "Signing session has failed, removing and retrying"
                );

                self.signing_state_machine.remove_signing_session(*session_id).await;
            } else {
                // It's still active, so we skip
                trace!(
                    session_id = hex::encode(&session_id),
                    "Signing session already exists and is active, skipping initiation"
                );

                return;
            }
        }

        // Create the sweep PSBT
        let sweep_psbt = match create_psbt_async(sweep_session, &mut self.btc_server).await {
            Ok(psbt) => {
                trace!(
                    session_id = hex::encode(&session_id),
                    "Successfully created sweep PSBT with {} inputs, {} outputs",
                    psbt.inputs.len(),
                    psbt.outputs.len()
                );
                psbt
            }
            Err(e) => {
                error!(session_id = hex::encode(&session_id), "Failed to create sweep PSBT: {}", e);
                return;
            }
        };

        // Serialize the PSBT for signing
        let psbt_bytes = sweep_psbt.serialize();

        info!(session_id = hex::encode(&session_id), "Initiating FROST signing for sweep PSBT");

        // Initiate FROST signing-session with the directly created PSBT
        if let Err(e) = self
            .signing_state_machine
            .initate_signing_session(session_id, psbt_bytes, SigningPsbtType::Sweep)
            .await
        {
            error!(
                session_id = hex::encode(&session_id),
                "Failed to initiate FROST signing for sweep PSBT: {}", e
            );
        } else {
            info!(session_id = hex::encode(&session_id), "Initiating FROST signing for sweep PSBT");
        }
    }

    pub async fn run(mut self) {
        loop {
            let request = btc_server_client::WalletSweepSessionUpdatesRequest {};
            let mut stream = match self
                .btc_server
                .subscribe_to_wallet_sweep_session_updates(request)
                .await
            {
                Ok(stream) => {
                    debug!(target: "consensus::authority::frost_task::WalletSweepTask", "Connected to wallet sweep session updates");

                    stream
                }
                Err(_e) => {
                    error!(target: "consensus::authority::frost_task::WalletSweepTask", "Failed to subscribe to wallet sweep session updates");

                    continue;
                }
            };

            pin_mut!(stream);

            while let Some(received) = stream.next().await {
                let session_update = match received {
                    Ok(message) => {
                        trace!(target: "consensus::authority::frost_task::WalletSweepTask", "Received message from wallet sweep session stream");

                        message
                    }
                    Err(e) => {
                        warn!(target: "consensus::authority::frost_task::WalletSweepTask", "Received error from wallet sweep session stream: {e}");

                        continue;
                    }
                };

                self.handle_wallet_sweep_session_update(session_update).await
            }

            warn!(target: "consensus::authority::frost_task::WalletSweepTask", "Wallet sweep session stream disconnected. Reconnect in 1 second");

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }
}
