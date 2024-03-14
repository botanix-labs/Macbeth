use clap::Parser;
use reth::{
    cli::{
        components::RethNodeComponents,
        ext::{NoArgs, NoArgsCliExt, RethNodeCommandConfig},
    },
    commands::poa::PoaNodeCommand,
    network::Peers,
    tasks::TaskSpawner,
};
use reth_primitives::{ChainSpec, BOTANIX_TESTNET};
use reth_provider::{CanonStateNotification, CanonStateSubscriptions};
use reth_rpc_types::PeerId;
use secp256k1::SecretKey;
use std::{
    collections::HashMap,
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{config::Config, suite::consensus::frost::btc_server::SpawnedBtcServer};

const RPC_PORT_BASE: u16 = 8545;
const AUTHRPC_PORT_BASE: u16 = 8551;
const DISCOVERY_PORT_BASE: u16 = 30303;
const FED_MEMBER1_SECRET_KEY: &'static str =
    "0a35afe1386497890e1dce7286a5b378b978ede20db900e6ce5b4eb1a0449ad6";
const FED_MEMBER2_SECRET_KEY: &'static str =
    "0cc8f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe057f094135f2c9b019";

pub fn is_inturn(authorities_len: u64, signer_index: u64) -> bool {
    // use minutes as time unit to determine in turn
    let timestamp = unix_timestamp() / 60;

    (timestamp / authorities_len) % authorities_len == signer_index
}

pub fn unix_timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[derive(Clone, Debug)]
pub struct ChannelPayload {
    pub engine_index: u16,
    pub ts: tokio::time::Instant,
    pub notification: CanonStateNotification,
}

#[derive(Clone, Debug)]
pub struct FederationMemberTestConfig {
    pub index: u16,
    pub temp_path: PathBuf,
    pub secret_key: SecretKey,
    pub rpc_port: u16,
    pub authrpc_port: u16,
    pub discovery_port: u16,
    pub bitcoind_url: String,
    pub bitcoind_username: String,
    pub bitcoind_password: String,
    pub bitcoin_server_url: String,
    pub peers_list: Vec<FederationMemberTestConfig>,
    pub sender: tokio::sync::mpsc::Sender<ChannelPayload>,
}

impl FederationMemberTestConfig {
    pub fn new(
        index: u16,
        secret_key: SecretKey,
        sender: tokio::sync::mpsc::Sender<ChannelPayload>,
        bitcoind_url: String,
        bitcoind_username: String,
        bitcoind_password: String,
        bitcoin_server_url: String,
    ) -> Self {
        Self {
            index,
            temp_path: tempfile::TempDir::new().expect("tempdir is okay").into_path(),
            secret_key,
            rpc_port: RPC_PORT_BASE + index,
            authrpc_port: AUTHRPC_PORT_BASE + index,
            discovery_port: DISCOVERY_PORT_BASE + index,
            bitcoind_url,
            bitcoind_username,
            bitcoind_password,
            bitcoin_server_url,
            peers_list: vec![],
            sender,
        }
    }

    pub fn insert_peers_list(&mut self, peers: Vec<FederationMemberTestConfig>) {
        self.peers_list = peers;
    }

    pub fn build_command(&self) -> PoaNodeCommand<NoArgsCliExt<FederationMemberTestConfig>> {
        println!("Engine {} data directory", self.index);
        println!(
            "Engine {} secret key = {}",
            self.index,
            &self.secret_key.display_secret().to_string()
        );

        let datadir = self.temp_path.to_str().expect("temp path is okay");
        let discovery_secret_path = Path::new(&self.temp_path).join("discovery-secret");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(discovery_secret_path.clone())
            .unwrap();
        file.write_all(&self.secret_key.display_secret().to_string().as_bytes()).unwrap();

        let no_args = NoArgs::with(self.clone());
        let mut command = PoaNodeCommand::<NoArgsCliExt<FederationMemberTestConfig>>::parse_from([
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
            "--authrpc.port",
            format!("{}", self.authrpc_port).as_str(),
            "--btc-server",
            self.bitcoin_server_url.as_str(),
            "--bitcoind.url",
            self.bitcoind_url.as_str(),
            "--bitcoind.username",
            self.bitcoind_username.as_str(),
            "--bitcoind.password",
            self.bitcoind_password.as_str(),
            "--port",
            format!("{}", self.discovery_port).as_str(),
            "--p2p-secret-key",
            discovery_secret_path.to_str().unwrap(),
        ])
        .with_ext::<NoArgsCliExt<FederationMemberTestConfig>>(no_args);

        // use custom chain spec
        command.chain = testnet_custom_chain();

        command
    }
}

impl RethNodeCommandConfig for FederationMemberTestConfig {
    fn on_node_started<Reth: RethNodeComponents>(&mut self, components: &Reth) -> eyre::Result<()> {
        println!("Engine {} started task", self.index);

        // add the peers
        for peer in self.peers_list.iter() {
            let peer_socket =
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), peer.discovery_port);
            components.network().add_peer(PeerId::random(), peer_socket);
        }
        println!("Engine {} added peers", self.index);

        let _pool = components.pool();
        let mut canon_events = components.events().subscribe_to_canonical_state();
        let rx_sender = self.sender.clone();
        let engine_index = self.index;

        components.task_executor().spawn(Box::pin(async move {
            // TODO: test the block creation per index
            while let Some(canon_event) = canon_events.recv().await.ok() {
                println!("Engine {} canonical tip", engine_index);
                let _ = rx_sender
                    .send(ChannelPayload {
                        engine_index,
                        ts: tokio::time::Instant::now(),
                        notification: canon_event,
                    })
                    .await;
            }
        }));

        Ok(())
    }
}

pub fn testnet_custom_chain() -> Arc<ChainSpec> {
    BOTANIX_TESTNET.clone()
}

pub fn create_poa_federation_members(
    config: &Config,
    btc_servers: Option<&Vec<SpawnedBtcServer>>,
) -> (HashMap<usize, FederationMemberTestConfig>, tokio::sync::mpsc::Receiver<ChannelPayload>) {
    // create two secret keys one for each member
    let sc1 = FED_MEMBER1_SECRET_KEY.parse::<SecretKey>().unwrap();
    let sc2 = FED_MEMBER2_SECRET_KEY.parse::<SecretKey>().unwrap();

    // create the member configs
    let (tx, rx) = tokio::sync::mpsc::channel::<ChannelPayload>(10);

    // create federation members
    let index: u16 = 0;
    let port = btc_servers
        .and_then(|servers| servers.iter().nth(index as usize).map(|val| val.port))
        .unwrap();
    let mut fed_member_config1 = FederationMemberTestConfig::new(
        index,
        sc1,
        tx.clone(),
        config.bitcoind.url.clone(),
        config.bitcoind.username.clone(),
        config.bitcoind.password.clone(),
        format!("http://localhost:{}", port),
    );

    let index: u16 = 1;
    let port = btc_servers
        .and_then(|servers| servers.iter().nth(index as usize).map(|val| val.port))
        .unwrap();
    let mut fed_member_config2 = FederationMemberTestConfig::new(
        index,
        sc2,
        tx,
        config.bitcoind.url.clone(),
        config.bitcoind.username.clone(),
        config.bitcoind.password.clone(),
        format!("http://localhost:{}", port),
    );

    // insert peers
    fed_member_config1.insert_peers_list(vec![fed_member_config2.clone()]);
    fed_member_config2.insert_peers_list(vec![fed_member_config1.clone()]);

    // persist all in a hashmap
    let mut fed_members: HashMap<usize, FederationMemberTestConfig> = HashMap::new();
    fed_members.insert(0, fed_member_config1);
    fed_members.insert(1, fed_member_config2);
    (fed_members, rx)
}
