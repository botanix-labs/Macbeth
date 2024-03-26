use clap::Parser;
use client::Empty;
use ethers::core::types::Address as EtherAddress;
use reth::{
    cli::{
        components::RethNodeComponents,
        ext::{NoArgs, NoArgsCliExt, RethNodeCommandConfig},
    },
    commands::poa::PoaNodeCommand,
    network::Peers,
    tasks::TaskSpawner,
    utils::get_or_create_jwt_secret_from_path,
};
use reth_authority_consensus::extended_client::BtcServerExtendedClient;
use reth_primitives::{ChainSpec, BOTANIX_TESTNET};
use reth_provider::{CanonStateNotification, CanonStateSubscriptions};
use reth_rpc_types::PeerId;
use secp256k1::SecretKey;
use std::{
    collections::HashMap,
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{config::Config, suite::consensus::frost::btc_server::SpawnedBtcServer};

use super::mint_contract::MintContractInstance;

const RPC_PORT_BASE: u16 = 8545;
const AUTHRPC_PORT_BASE: u16 = 8551;
const DISCOVERY_PORT_BASE: u16 = 30303;
const FED_MEMBER1_SECRET_KEY: &'static str =
    "0a35afe1386497890e1dce7286a5b378b978ede20db900e6ce5b4eb1a0449ad6";
const FED_MEMBER2_SECRET_KEY: &'static str =
    "0cc8f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe057f094135f2c9b019";
const MINT_CONTRACT_ADDRESS: &'static str = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2";

pub fn is_inturn(authorities_len: u64, signer_index: u64) -> bool {
    // use minutes as time unit to determine in turn
    let timestamp = unix_timestamp() / 60;
    (timestamp / authorities_len) % authorities_len == signer_index
}

pub fn unix_timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[derive(Clone, Debug)]
pub enum Notifications {
    CanonState(CannonStateNofificationPayload),
    DkgFinished(DkgPayload),
}

#[derive(Clone, Debug)]
pub struct DkgPayload {
    pub engine_index: u16,
    pub ts: tokio::time::Instant,
}

#[derive(Clone, Debug)]
pub struct CannonStateNofificationPayload {
    pub engine_index: u16,
    pub ts: tokio::time::Instant,
    pub notification: CanonStateNotification,
}

#[derive(Clone, Debug)]
pub struct FederationMemberTestConfig {
    pub index: u16,
    pub temp_path: PathBuf,
    pub secret_key: String,
    pub rpc_port: u16,
    pub authrpc_port: u16,
    pub discovery_port: u16,
    pub bitcoind_url: String,
    pub bitcoind_username: String,
    pub bitcoind_password: String,
    pub bitcoin_server_url: String,
    pub peers_list: Vec<FederationMemberTestConfig>,
    pub sender: tokio::sync::mpsc::Sender<Notifications>,
    pub jwt_secret_path: PathBuf,
    pub frost_min_signers: u16,
    pub frost_max_signers: u16,
    pub peer_id: PeerId,
    pub is_dkg_ready: bool,
}

impl FederationMemberTestConfig {
    pub async fn new(
        index: u16,
        secret_key: String,
        sender: tokio::sync::mpsc::Sender<Notifications>,
        bitcoind_url: String,
        bitcoind_username: String,
        bitcoind_password: String,
        bitcoin_server_url: String,
        jwt_secrets_dir: PathBuf,
        frost_min_signers: u16,
        frost_max_signers: u16,
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
            bitcoind_url,
            bitcoind_username,
            bitcoind_password,
            bitcoin_server_url,
            peers_list: vec![],
            sender,
            jwt_secret_path,
            frost_min_signers,
            frost_max_signers,
            peer_id,
            is_dkg_ready: false,
        }
    }

    pub async fn create_mint_contract_instance(&self) -> MintContractInstance {
        let mint_contract_address: EtherAddress =
            MINT_CONTRACT_ADDRESS.parse().expect("Must be a valid ethereum address");
        MintContractInstance::new(self.rpc_port, &self.secret_key, mint_contract_address).await
    }

    pub fn insert_peers_list(&mut self, peers: Vec<FederationMemberTestConfig>) {
        self.peers_list = peers;
    }

    pub fn is_dkg_ready(&self) -> bool {
        self.is_dkg_ready
    }

    pub fn build_command(&self) -> PoaNodeCommand<NoArgsCliExt<FederationMemberTestConfig>> {
        println!("Engine {} data directory", self.index);
        println!("Engine {} secret key = {}", self.index, &self.secret_key);

        let datadir = self.temp_path.to_str().expect("temp path is okay");
        let discovery_secret_path = Path::new(&self.temp_path).join("discovery-secret");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(discovery_secret_path.clone())
            .unwrap();
        file.write_all(&self.secret_key.as_bytes()).unwrap();

        let jwt_secret_path = self.jwt_secret_path.display().to_string();

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
            "--authrpc.jwtsecret",
            jwt_secret_path.as_str(),
            "--btc-server",
            self.bitcoin_server_url.as_str(),
            "--bitcoind.url",
            self.bitcoind_url.as_str(),
            "--bitcoind.username",
            self.bitcoind_username.as_str(),
            "--bitcoind.password",
            self.bitcoind_password.as_str(),
            "--frost.min_signers",
            self.frost_min_signers.to_string().as_str(),
            "--frost.max_signers",
            self.frost_max_signers.to_string().as_str(),
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
            components.network().add_peer(self.peer_id, peer_socket);
        }
        println!("Engine {} added peers", self.index);

        let _pool = components.pool();
        let mut canon_events = components.events().subscribe_to_canonical_state();
        let rx_sender = self.sender.clone();
        let engine_index = self.index;

        let bitcoin_server_url = self.bitcoin_server_url.clone();
        let jwt_secret_path = self.jwt_secret_path.clone();

        components.task_executor().spawn(Box::pin(async move {
            // create a btc client
            let jwt_secret = get_or_create_jwt_secret_from_path(&jwt_secret_path).unwrap();
            let mut btc_server_client = BtcServerExtendedClient::new(
                format!("http://{}", bitcoin_server_url),
                Some(jwt_secret),
            )
            .await
            .unwrap();

            // wait for the dkg to finish
            loop {
                match btc_server_client.get_public_key(Empty {}).await {
                    Ok(_) => {
                        println!("Dkg Finished !");
                        break;
                    }
                    Err(_) => {
                        println!("Dkg Pending...");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
            }
            let _ = rx_sender
                .send(Notifications::DkgFinished(DkgPayload {
                    engine_index,
                    ts: tokio::time::Instant::now(),
                }))
                .await;

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

pub fn testnet_custom_chain() -> Arc<ChainSpec> {
    BOTANIX_TESTNET.clone()
}

pub fn is_dkg_ready(federation_memebers: &HashMap<usize, FederationMemberTestConfig>) -> bool {
    !federation_memebers.iter().any(|(_, member)| !member.is_dkg_ready())
}

pub async fn create_poa_federation_members(
    config: &Config,
    btc_servers: Option<&Vec<SpawnedBtcServer>>,
) -> (HashMap<usize, FederationMemberTestConfig>, tokio::sync::mpsc::Receiver<Notifications>) {
    let peer_id_1 = PeerId::from_str("bdc272b244f717604fffe659d2d98205d1e6764fdf453d1631f42c2db4d8d710606084da81495d55673bfc038bdf41e3f4c17d09c875a0bcc1ea809219e34826").unwrap();
    let peer_id_2 = PeerId::from_str("9bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d50a117189201f0ad9096d36cd690ae34e79a42d9e71c972e55048dabdc8f9651").unwrap();

    // create the member configs
    let (tx, rx) = tokio::sync::mpsc::channel::<Notifications>(100);

    // create federation members
    let index: u16 = 0;
    let port = btc_servers
        .and_then(|servers| servers.iter().nth(index as usize).map(|val| val.port))
        .unwrap();
    let mut fed_member_config1 = FederationMemberTestConfig::new(
        index,
        FED_MEMBER1_SECRET_KEY.to_string(),
        tx.clone(),
        config.bitcoind.url.clone(),
        config.bitcoind.username.clone(),
        config.bitcoind.password.clone(),
        format!("localhost:{}", port),
        config.jwt_secrets_dir.clone(),
        config.frost_min_signers.clone(),
        config.frost_max_signers.clone(),
        peer_id_1,
    )
    .await;

    let index: u16 = 1;
    let port = btc_servers
        .and_then(|servers| servers.iter().nth(index as usize).map(|val| val.port))
        .unwrap();
    let mut fed_member_config2 = FederationMemberTestConfig::new(
        index,
        FED_MEMBER2_SECRET_KEY.to_string(),
        tx,
        config.bitcoind.url.clone(),
        config.bitcoind.username.clone(),
        config.bitcoind.password.clone(),
        format!("localhost:{}", port),
        config.jwt_secrets_dir.clone(),
        config.frost_min_signers.clone(),
        config.frost_max_signers.clone(),
        peer_id_2,
    )
    .await;

    // insert peers
    fed_member_config1.insert_peers_list(vec![fed_member_config2.clone()]);
    fed_member_config2.insert_peers_list(vec![fed_member_config1.clone()]);

    // persist all in a hashmap
    let mut fed_members: HashMap<usize, FederationMemberTestConfig> = HashMap::new();
    fed_members.insert(0, fed_member_config1);
    fed_members.insert(1, fed_member_config2);
    (fed_members, rx)
}
