use crate::it_info_print;
use crate::suite::consensus::{
    frost::poa_node::{CannonStateNofificationPayload, FederationMemberTestConfig, Notifications},
    GlobalContext,
};
use reth::{
    cli::{components::RethNodeComponents, ext::RethNodeCommandConfig},
    network::Peers,
    tasks::TaskSpawner,
};
use reth_ecies::util::pk2id;
use reth_primitives::hex::encode as hex_encode;
use reth_provider::CanonStateSubscriptions;
use reth_rpc_types::PeerId;
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};
use url::Url;

const RPC_PORT_BASE: u16 = 8545;
const AUTHRPC_PORT_BASE: u16 = 8551;
const DISCOVERY_PORT_BASE: u16 = 30321;
const PREFUNDED_ACCOUNT_SECRET_KEY: &'static str =
    "52947524bbc14bd90cc86c32b9b7564da2f7f8de343825fed68cd04da4925d29";

#[derive(Clone, Debug)]
pub struct NonFederationMemberTestConfig {
    pub index: u16,
    pub temp_path: PathBuf,
    pub secret_key: String,
    pub rpc_port: u16,
    pub authrpc_port: u16,
    pub discovery_port: u16,
    pub bitcoind_url: Url,
    pub bitcoind_username: String,
    pub bitcoind_password: String,
    pub peers_list: Vec<FederationMemberTestConfig>,
    pub sender: tokio::sync::mpsc::Sender<Notifications>,
    pub jwt_secret_path: PathBuf,
    pub peer_id: PeerId,
}

impl NonFederationMemberTestConfig {
    pub fn new(
        index: u16,
        secret_key: String,
        sender: tokio::sync::mpsc::Sender<Notifications>,
        bitcoind_url: Url,
        bitcoind_username: String,
        bitcoind_password: String,
        jwt_secrets_dir: PathBuf,
        peer_id: PeerId,
    ) -> Self {
        let rpc_port = RPC_PORT_BASE + index;
        let authrpc_port = AUTHRPC_PORT_BASE + index;
        let discovery_port = DISCOVERY_PORT_BASE + index;
        let jwt_secret_path = jwt_secrets_dir.join(format!("{}.hex", index + 1));
        Self {
            index,
            temp_path: tempfile::TempDir::new().expect("tempdir is okay").into_path(),
            secret_key,
            rpc_port,
            authrpc_port,
            discovery_port,
            bitcoind_url: Url::parse("http://localhost:18443").unwrap(),
            bitcoind_username,
            bitcoind_password,
            peers_list: vec![],
            sender,
            jwt_secret_path,
            peer_id,
        }
    }

    pub fn insert_peers_list(&mut self, peers: Vec<FederationMemberTestConfig>) {
        self.peers_list = peers;
    }
}

impl RethNodeCommandConfig for NonFederationMemberTestConfig {
    fn on_node_started<Reth: RethNodeComponents>(&mut self, components: &Reth) -> eyre::Result<()> {
        it_info_print!("Engine started non federation task with index: ", self.index);

        let _pool = components.pool();
        let mut canon_events = components.events().subscribe_to_canonical_state();
        let rx_sender = self.sender.clone();
        let engine_index = self.index;

        let peers_list = self.peers_list.clone();
        let comp = components.clone();
        components.task_executor().spawn(Box::pin(async move {
            // add the peers
            'inner: loop {
                for peer in peers_list.iter() {
                    let peer_socket = SocketAddr::new(
                        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                        peer.discovery_port,
                    );
                    comp.network().add_peer(peer.peer_id, peer_socket);
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let all_peers = comp.network().get_all_peers().await.unwrap();
                it_info_print!(
                    "Engine connected with peers",
                    format!("index={}: peers_count={}", engine_index, all_peers.len())
                );
                if all_peers.len() == peers_list.len() {
                    break 'inner;
                }
            }

            // start waiting for canon event notifications
            while let Some(canon_state_notification) = canon_events.recv().await.ok() {
                let _ = rx_sender
                    .send(Notifications::CanonState(CannonStateNofificationPayload {
                        engine_index,
                        ts: tokio::time::Instant::now(),
                        notification: canon_state_notification,
                    }))
                    .await;
            }
        }));

        Ok(())
    }
}

pub async fn create_rpc_node(
    global_context: Arc<GlobalContext>,
    federation_members: HashMap<u16, FederationMemberTestConfig>,
) -> (NonFederationMemberTestConfig, tokio::sync::mpsc::Receiver<Notifications>) {
    let (tx, rx) = tokio::sync::mpsc::channel::<Notifications>(100);

    let secp = secp256k1::Secp256k1::new();
    let secret_key = secp256k1::SecretKey::new(&mut rand::thread_rng());
    let pk = secp256k1::PublicKey::from_secret_key(&secp, &secret_key);
    let rpc_secret_key = hex_encode(secret_key.as_ref());
    let rpc_peer_id = pk2id(&pk);

    // set index as federation_members length + 1:
    // this will ensure the correct ports are used
    let index = (federation_members.len() as u16) + 1;
    let mut rpc_node = NonFederationMemberTestConfig::new(
        index,
        rpc_secret_key,
        tx.clone(),
        global_context.bitcoind_url.clone(),
        global_context.bitcoind_user.clone(),
        global_context.bitcoind_pass.clone(),
        global_context.jwt_dir.clone(),
        rpc_peer_id,
    );

    // insert federation members as peers
    rpc_node.insert_peers_list(federation_members.values().cloned().collect::<Vec<_>>());

    (rpc_node, rx)
}
