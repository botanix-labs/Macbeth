use crate::{dkg::DKGStateMachine, epoch_manager::EpochManager};
use client::BtcServerClient;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_network::{
    frost::{
        manager::{FrostCommand, FrostConfig, FrostHandle},
        EventResponseType, Response,
    },
    NetworkHandle,
};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use tracing::{error, info};

pub struct FrostTask<Client> {
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    /// Frost Handler
    pub(crate) frost_handle: FrostHandle,
    /// Epoch manager
    pub(crate) epoch_manager: EpochManager<Client>,
    /// dkg state machine
    pub(crate) dkg_state_machine: DKGStateMachine,
}

impl<Client> FrostTask<Client>
where
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        btc_server: BtcServerClient<tonic::transport::Channel>,
        network_handle: NetworkHandle,
        frost_handle: FrostHandle,
        epoch_manager: EpochManager<Client>,
        config: FrostConfig,
    ) -> Self {
        let FrostConfig { authority_index, total_authorities, min_signers, max_signers } = config;
        info!("Frost authority index: {}/{}", authority_index, total_authorities);

        let dkg_state_machine = DKGStateMachine::new(
            btc_server,
            frost_handle.clone(),
            authority_index as u16,
            min_signers,
            max_signers,
        );

        Self { network_handle, frost_handle, epoch_manager, dkg_state_machine }
    }

    async fn start_dkg(&mut self) {
        // check if we are connected to all frost peers when in turn
        let (sender, receiver) = tokio::sync::oneshot::channel::<bool>();
        self.frost_handle.send_command(FrostCommand::CheckConnectedToAll(sender));
        match receiver.await {
            Ok(is_connected) => {
                if !is_connected {
                    info!("Not yet connected to all frost peers. Waiting ....");
                    return;
                }
                info!(">>>>>>>>>>> [FROST_TASK] Connected to all frost peers {}", is_connected);
                // start the dkg process / restart ?
                info!(">>>>>>>>>>> [FROST_TASK] Starting the DKG state machine...");
                let _ = self.dkg_state_machine.start().await;
            }
            Err(e) => {
                error!("Check for connection to other peers failed {:?}", e);
            }
        }
    }

    pub async fn start_task(&mut self) -> () {
        // before we start get a proper event receiver
        let (peer_messages_tx, peer_messages_rx) = tokio::sync::oneshot::channel();
        self.frost_handle.send_command(FrostCommand::GetPeerMessagesStream(peer_messages_tx));
        let mut peer_messages_rx = match peer_messages_rx.await {
            Ok(peer_messages_rx) => peer_messages_rx,
            Err(e) => {
                error!("Error getting receiver handle = {:?}", e);
                panic!("Error getting receiver handle");
            }
        };

        loop {
            // Check if we are in_turn and if we need to run the dkg start process
            let is_inturn = self.epoch_manager.poll().await;

            // start dkg only when we are in turn + initial state + no public key
            if is_inturn &&
                !self.dkg_state_machine.get_dkg_state().is_running() &&
                self.dkg_state_machine.get_public_key().await.is_err()
            {
                self.start_dkg().await;
            }

            // receive over a channel message from other peers and update our state machine
            if let Ok(msg) = peer_messages_rx.try_recv() {
                info!(">>>>>>>>>>> [FROST_TASK] Peer messaged received {:?}", msg);
                let Response { response_type, identifier, data } = msg;
                match response_type {
                    EventResponseType::DkgRound1 => {
                        match self.dkg_state_machine.process_round1(identifier, data).await {
                            Ok(_) => {
                                info!(">>>>>>>>>>> [FROST_TASK] Processed Round 1 package successfully")
                            }
                            Err(e) => {
                                error!(">>>>>>>>>>> [FROST_TASK] Error processing round 1 package {:?}", e);
                            }
                        }
                    }
                    EventResponseType::DkgRound2 => {
                        match self.dkg_state_machine.process_round2(identifier, data).await {
                            Ok(_) => {
                                info!(">>>>>>>>>>> [FROST_TASK] Processed Round 2 package successfully")
                            }
                            Err(e) => {
                                error!(">>>>>>>>>>> [FROST_TASK] Error processing round 2 package {:?}", e);
                            }
                        }
                    }
                }
            }

            // short sleep
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
}

impl<Client> std::fmt::Debug for FrostTask<Client>
where
    Client: Clone + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrostTask").finish_non_exhaustive()
    }
}
