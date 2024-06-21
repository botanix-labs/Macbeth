use std::time::Duration;

use crate::{
    it_info_print,
    suite::consensus::{common::poa_node::TestSignal, ConsensusIntegrationTestSuite},
};
use reth::network::PeerInfo;

#[allow(clippy::too_many_lines)]
pub async fn frost_e2e_peer_disconnect(
    suite: &ConsensusIntegrationTestSuite,
) -> Result<(), super::error::Error> {
    // get the federation members
    let test_fed_members = suite
        .local_context
        .poa_nodes
        .as_ref()
        .expect("test federation member configurations")
        .clone();
    let _rx = suite.local_context.poa_notification.as_ref().expect("poa notifs").subscribe();

    // now disconnect the peers of fed member 0
    test_fed_members.get(&0).cloned().unwrap().send_test_signal(TestSignal::DisconnectAll());

    // wait for standard ping time of 30 seconds then fed member 0 should have re-connected with all
    // its peers again
    tokio::time::sleep(Duration::from_secs(30 + 5)).await;

    // start looping and check that test fed members has now been connected with all peers again
    let (signals_tx, mut signals_rx) = tokio::sync::broadcast::channel::<Vec<PeerInfo>>(2);
    test_fed_members
        .get(&0)
        .cloned()
        .unwrap()
        .send_test_signal(TestSignal::GetAllPeers(signals_tx));
    while let Ok(all_connected_peers) = signals_rx.recv().await {
        it_info_print!("got all connected peers {:?}", all_connected_peers);
    }

    Ok(())
}
