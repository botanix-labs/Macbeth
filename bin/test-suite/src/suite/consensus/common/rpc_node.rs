use crate::{
    it_info_print,
    suite::consensus::{
        common::poa_node::{
            CannonStateNofificationPayload, FederationMemberTestConfig, Notifications,
        },
        GlobalContext,
    },
};
use clap::Parser;
use reth::{
    cli::ext::{NoArgs, PoaNodeCommandConfig, RethNodeComponents},
    commands::poa::PoaNodeCommand,
    consensus_common::utils::unix_timestamp,
    network::Peers,
};
use reth_network_types::pk2id;
use reth_primitives::{hex::encode as hex_encode, ChainSpec};
use reth_provider::CanonStateSubscriptions;
use reth_rpc_types::PeerId;
use std::{
    collections::HashMap,
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
};
use url::Url;

const RPC_PORT_BASE: u16 = 8545;
const AUTHRPC_PORT_BASE: u16 = 8551;
const DISCOVERY_PORT_BASE: u16 = 30321;

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
    #[allow(clippy::too_many_arguments)]
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
        let jwt_secret_path = jwt_secrets_dir.join(format!("{index}.hex"));
        Self {
            index,
            temp_path: {
                let ret = tempfile::TempDir::new()
                    .expect("tempdir is okay")
                    .into_path()
                    .join(format!("_{}", unix_timestamp().to_string()));
                std::fs::create_dir_all(&ret).expect("can't create tmpdir subdir");
                ret
            },
            secret_key,
            rpc_port,
            authrpc_port,
            discovery_port,
            bitcoind_url,
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

    pub fn build_command(
        &self,
        chain_spec: ChainSpec,
    ) -> PoaNodeCommand<NoArgs<NonFederationMemberTestConfig>> {
        it_info_print!(format!("RPC Engine {} secret key = {}", self.index, &self.secret_key));

        let datadir = self.temp_path.to_str().expect("temp path is okay");
        let discovery_secret_path = Path::new(&self.temp_path).join("discovery-secret");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(discovery_secret_path.clone())
            .expect("file can be opened");
        file.write_all(&self.secret_key.as_bytes()).expect("secret key written to file");

        let jwt_secret_path = self.jwt_secret_path.display().to_string();

        let no_args = NoArgs::with(self.clone());
        let mut command = PoaNodeCommand::<NoArgs<FederationMemberTestConfig>>::parse_from([
            "poa",
            "--chain",
            "botanix_testnet",
            "--datadir",
            datadir,
            "--debug.terminate",
            "--http",
            "--http.corsdomain",
            "*",
            "--http.port",
            format!("{}", self.rpc_port).as_str(),
            "--http.addr",
            "127.0.0.1",
            "--http.api",
            "eth,net,trace,txpool,web3,rpc,admin",
            "--authrpc.addr",
            "127.0.0.1",
            "--btc-network",
            "regtest",
            "--authrpc.port",
            format!("{}", self.authrpc_port).as_str(),
            "--authrpc.jwtsecret",
            jwt_secret_path.as_str(),
            "--bitcoind.url",
            self.bitcoind_url.as_str(),
            "--bitcoind.username",
            self.bitcoind_username.as_str(),
            "--bitcoind.password",
            self.bitcoind_password.as_str(),
            "--port",
            format!("{}", self.discovery_port).as_str(),
            "--p2p-secret-key",
            discovery_secret_path.to_str().expect("discovery secret path to exist"),
        ])
        .with_ext::<NoArgs<NonFederationMemberTestConfig>>(no_args);
        command.chain = Arc::new(chain_spec);

        command
    }
}

impl PoaNodeCommandConfig for NonFederationMemberTestConfig {
    #[allow(clippy::unwrap_used)]
    fn on_node_started(&self, components: RethNodeComponents) -> eyre::Result<()> {
        it_info_print!("Engine started non federation task with index: ", self.index);

        let RethNodeComponents { executor, db, network } = components;

        let mut canon_events = db.subscribe_to_canonical_state();
        let rx_sender = self.sender.clone();
        let engine_index = self.index;

        let peers_list = self.peers_list.clone();
        it_info_print!("RPC Engine peers list", peers_list.len());

        executor.spawn(Box::pin(async move {
            // add the peers
            'inner: loop {
                for peer in peers_list.iter() {
                    let peer_socket = SocketAddr::new(
                        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                        peer.discovery_port,
                    );
                    network.add_peer(peer.peer_id, peer_socket);
                    it_info_print!("RPC added peer", peer.peer_id);
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let all_peers = network.get_all_peers().await.unwrap();
                it_info_print!(
                    "RPC Engine connected with peers",
                    format!("index={}: peers_count={}", engine_index, all_peers.len())
                );
                if all_peers.len() == peers_list.len() {
                    break 'inner;
                }
            }

            // start waiting for canon event notifications
            while let Ok(canon_state_notification) = canon_events.recv().await {
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

#[allow(clippy::cast_possible_truncation)]
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
    let index = federation_members.len() as u16 + 1;
    let mut rpc_node = NonFederationMemberTestConfig::new(
        index,
        rpc_secret_key,
        tx,
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
