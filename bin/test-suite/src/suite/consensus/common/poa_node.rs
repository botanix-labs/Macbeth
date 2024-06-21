
use std::{
    collections::HashMap,
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration},
};

use askama::Template;
use bitcoin::{BlockHash, hashes::{sha256, Hash}};
use clap::Parser;
use client::{Empty, GetSessionIdsRequest, GetSigningStatusRequest, SigningStatus};
use ethers::core::types::Address as EtherAddress;
use reth::{
    args::FedMemberPubKey,
    cli::ext::{NoArgs, PoaNodeCommandConfig, RethNodeComponents},
    commands::poa::PoaNodeCommand,
    consensus_common::utils::unix_timestamp,
    network::{PeerKind, Peers},
    utils::get_or_create_jwt_secret_from_path,
};
use reth_authority_consensus::extended_client::BtcServerExtendedClient;
use reth_network_types::pk2id;
use reth_node_core::args::GenesisTomlConfig;
use reth_primitives::{
    create_botanix_config_with_genesis,
    extra_data_header::{ExtraDataHeader, EXTRA_HEADER_VERSION},
    ChainSpec,
};
use reth_provider::{CanonStateNotification, CanonStateSubscriptions};
use reth_rpc_types::PeerId;
use secp256k1::{PublicKey, SecretKey, SECP256K1};
use tokio::sync::broadcast::{channel, Sender};
use url::Url;

use super::{botanix_client::BotanixEthClient, btc_server::SpawnedBtcServer};
use crate::{context::GlobalContext, it_info_print, it_warn_print};

const MINT_CONTRACT_ADDRESS: &'static str = "0x0Ea320990B44236A0cEd0ecC0Fd2b2df33071e78";
pub const PREFUNDED_ACCOUNT_SECRET_KEY: &'static str =
    "52947524bbc14bd90cc86c32b9b7564da2f7f8de343825fed68cd04da4925d29";

#[derive(Template, Clone, Debug)]
#[template(path = "botanix_testnet.json", ext = "json", escape = "none")]
struct BotanixTestnetGenesisConfig<'a> {
    edh: &'a str,
}

#[derive(Clone, Debug)]
pub enum Notifications {
    CanonState(CannonStateNofificationPayload),
    DkgFinished(DkgPayload),
    SigningStatusReport((u16, Vec<u8>, SigningStatus)),
}

#[derive(Clone, Debug)]
pub enum TestSignal {
    DisconnectAll(),
    ReconnectAll(),
}

#[derive(Clone, Debug)]
pub struct DkgPayload {
    pub engine_index: u16,
    pub ts: tokio::time::Instant,
    pub public_key: String,
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
    pub secret_key: SecretKey,
    pub authorities: Vec<PublicKey>,
    pub rpc_port: u16,
    pub discovery_port: u16,
    pub bitcoind_url: Url,
    pub bitcoind_username: String,
    pub bitcoind_password: String,
    pub bitcoin_server_url: String,
    pub peers_list: Vec<FederationMemberTestConfig>,
    pub sender: tokio::sync::broadcast::Sender<Notifications>,
    pub jwt_secret_path: PathBuf,
    pub frost_min_signers: u16,
    pub frost_max_signers: u16,
    pub peer_id: PeerId,
    pub is_dkg_ready: bool,
    pub edh: Option<ExtraDataHeader>,
    pub test_signal_tx: Sender<TestSignal>,
}

impl FederationMemberTestConfig {
    pub async fn new(
        index: u16,
        secret_key: SecretKey,
        authorities: Vec<PublicKey>,
        sender: tokio::sync::broadcast::Sender<Notifications>,
        bitcoind_url: Url,
        bitcoind_username: String,
        bitcoind_password: String,
        bitcoin_server_url: String,
        jwt_secrets_dir: PathBuf,
        frost_min_signers: u16,
        frost_max_signers: u16,
        peer_id: PeerId,
        rpc_port_base: u16,
        discovery_port_base: u16,
        test_signal_tx: Sender<TestSignal>,
    ) -> Self {
        let rpc_port = rpc_port_base + index;
        let discovery_port = discovery_port_base + index;
        let jwt_secret_path = jwt_secrets_dir.join(format!("{}.hex", index + 1));
        Self {
            index,
            temp_path: {
                let ret = tempfile::TempDir::new()
                    .expect("tempdir is okay")
                    .into_path()
                    .join(format!("_{}", unix_timestamp().to_string()));
                std::fs::create_dir_all(&ret).expect("failed to create tempdir subdir");
                ret
            },
            secret_key,
            authorities,
            rpc_port,
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
            edh: None,
            test_signal_tx,
        }
    }

    pub async fn create_botanix_eth_client(&self) -> BotanixEthClient {
        let mint_contract_address: EtherAddress =
            MINT_CONTRACT_ADDRESS.parse().expect("Must be a valid ethereum address");
        BotanixEthClient::new(self.rpc_port, PREFUNDED_ACCOUNT_SECRET_KEY, mint_contract_address)
            .await
    }

    pub fn insert_peers_list(&mut self, peers: Vec<FederationMemberTestConfig>) {
        self.peers_list = peers;
    }

    pub fn insert_edh(&mut self, edh: ExtraDataHeader) {
        self.edh = Some(edh);
    }

    pub fn is_dkg_ready(&self) -> bool {
        self.is_dkg_ready
    }

    pub fn send_test_signal(&self, signal: TestSignal) {
        let _ = self.test_signal_tx.send(signal);
    }

    pub fn build_command(
        &self,
        edh_authorities_list: Arc<Vec<PublicKey>>,
    ) -> (PoaNodeCommand<NoArgs<FederationMemberTestConfig>>, ChainSpec) {
        // print secret key
        it_info_print!(format!("sk: {:?}", self.secret_key));
        it_info_print!(format!("Building federation member index: {}", self.index));

        let datadir = self.temp_path.to_str().expect("temp path is okay");
        let discovery_secret_path = Path::new(&self.temp_path).join("discovery-secret");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(discovery_secret_path.clone())
            .unwrap();
        file.write_all(&self.secret_key.display_secret().to_string().as_bytes()).unwrap();

        let jwt_secret_path = self.jwt_secret_path.display().to_string();

        // update genesis config with edh and render file
        let botanix_testnet_config_genesis = if let Some(edh) = self.edh.as_ref() {
            let edh = hex::encode(edh.serialize());
            let botanix_testnet_config_genesis = BotanixTestnetGenesisConfig { edh: &edh };
            let rendered_json = botanix_testnet_config_genesis.render().unwrap();
            rendered_json
        } else {
            panic!("Edh data missing. Cannot create botanix testnet config genesis file");
        };

        // Need to create a chain.toml in the data dir

        // Need to zip together the soc address and pk
        let mut fed_member_pks = vec![];
        for peer in self.peers_list.iter() {
            let pk = FedMemberPubKey {
                key: peer.secret_key.public_key(SECP256K1).to_string(),
                socket_addr: format!("127.0.0.1:{}", peer.discovery_port),
            };
            fed_member_pks.push(pk);
        }
        // add our selves
        let my_pk = FedMemberPubKey {
            key: self.secret_key.public_key(SECP256K1).to_string(),
            socket_addr: format!("127.0.0.1:{}", self.discovery_port),
        };
        fed_member_pks.push(my_pk);

        // NOTE: fed members have already created their EDH with the correct authorities
        // but the order may not be the same as fed_member_pks since we added ourselves last
        // so compare the EDH authorities list and build a new list in the correct order
        let mut edh_authorities = vec![];
        for authority in edh_authorities_list.to_vec().iter() {
            for pk in fed_member_pks.iter() {
                if pk.key == authority.to_string() {
                    edh_authorities.push(pk.clone());
                    break;
                }
            }
        }

        let chain_config =
            GenesisTomlConfig::new("integration test toml".to_string(), edh_authorities, None);
        it_info_print!("Chain config", chain_config);
        chain_config.write_to_path(Path::new(datadir).join("chain.toml")).unwrap();

        let no_args = NoArgs::with(self.clone());
        let mut command = PoaNodeCommand::<NoArgs<FederationMemberTestConfig>>::parse_from([
            "poa",
            "--chain",
            "botanix_testnet",
            "--federation-mode",
            "--ipcdisable",
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
            "--btc-network",
            "regtest",
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
        .with_ext::<NoArgs<FederationMemberTestConfig>>(no_args);

        // use botanix chain spec
        let genesis = serde_json::from_str(&botanix_testnet_config_genesis)
            .expect("Can't deserialize Botanix Testnet genesis json");
        let botanix_testnet = create_botanix_config_with_genesis(genesis, 6);
        command.chain = Arc::new(botanix_testnet.clone());

        (command, botanix_testnet)
    }
}

impl PoaNodeCommandConfig for FederationMemberTestConfig {
    fn on_node_started(&self, components: RethNodeComponents) -> eyre::Result<()> {
        it_info_print!("Engine started task with index: ", self.index);

        let RethNodeComponents { executor, db, network } = components;
        let network_clone = network.clone();

        let mut canon_events = db.subscribe_to_canonical_state();
        let engine_index = self.index;
        let rx_sender = self.sender.clone();
        let bitcoin_server_url = self.bitcoin_server_url.clone();
        let jwt_secret_path = self.jwt_secret_path.clone();
        let peers_list = self.peers_list.clone();

        // ~~~~~~~~~~ spawn initial task that adds peers and awaits dkg to finish ~~~~~~~~~~~
        executor.spawn(Box::pin(async move {
            // add the peers
            'inner: loop {
                for peer in peers_list.iter() {
                    let peer_socket = SocketAddr::new(
                        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                        peer.discovery_port,
                    );
                    network.add_peer(peer.peer_id, peer_socket);
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let all_peers = network.get_all_peers().await.unwrap();
                it_info_print!(
                    "Engine connected with peers",
                    format!("index={}: peers_count={}", engine_index, all_peers.len())
                );
                if all_peers.len() == peers_list.len() {
                    break 'inner;
                }
            }

            // create a btc client
            let jwt_secret = get_or_create_jwt_secret_from_path(&jwt_secret_path).unwrap();
            let mut btc_server_client = BtcServerExtendedClient::new(
                format!("http://{}", bitcoin_server_url),
                Some(jwt_secret),
            )
            .await
            .unwrap();

            // wait for the dkg to finish
            let pub_key = loop {
                match btc_server_client.get_public_key(Empty {}).await {
                    Ok(pub_key) => {
                        it_info_print!("Dkg Finished for index {:?}!", engine_index);
                        break pub_key;
                    }
                    Err(_) => {
                        it_warn_print!("Dkg Pending for engine index {:?}...", engine_index);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
            };
            let _ = rx_sender
                .send(Notifications::DkgFinished(DkgPayload {
                    engine_index,
                    ts: tokio::time::Instant::now(),
                    public_key: pub_key.publickey,
                }))
                .unwrap();
        }));

        // ~~~~~~~~~~~ spawn a task that loops and sends over channel all received canon state
        // notifications ~~~~~~~~~~~
        let rx_sender = self.sender.clone();
        executor.spawn(Box::pin(async move {
            // start waiting for canon event notifications
            while let Some(canon_state_notification) = canon_events.recv().await.ok() {
                match rx_sender.send(Notifications::CanonState(CannonStateNofificationPayload {
                    engine_index,
                    ts: tokio::time::Instant::now(),
                    notification: canon_state_notification,
                })) {
                    Ok(_) => {}
                    // all receivers have been dropped temporarily here. Just sleep and await new
                    // ones to be created
                    Err(_) => {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
            }
        }));

        // ~~~~~~~~~~~ spawn a task awaiting test signals from the test suite ~~~~~~~~~~~
        let mut receiver = self.test_signal_tx.subscribe();
        let peers_list = self.peers_list.clone();
        executor.spawn(Box::pin(async move {
            while let Ok(test_signal) = receiver.recv().await {
                match test_signal {
                    TestSignal::DisconnectAll() => {
                        // disconnect all peers
                        'inner: loop {
                            for peer in peers_list.iter() {
                                network_clone.remove_peer(peer.peer_id, PeerKind::Basic);
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            let all_peers = network_clone.get_all_peers().await.unwrap();
                            it_info_print!(
                                "Engine disconnected from peers",
                                format!("index={}: peers_count={}", engine_index, all_peers.len())
                            );
                            if all_peers.len() == 0 {
                                break 'inner;
                            }
                        }
                    }
                    TestSignal::ReconnectAll() => {
                        // re-add the peers
                        'inner: loop {
                            for peer in peers_list.iter() {
                                let peer_socket = SocketAddr::new(
                                    IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                                    peer.discovery_port,
                                );
                                network_clone.add_peer(peer.peer_id, peer_socket);
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            let all_peers = network_clone.get_all_peers().await.unwrap();
                            it_info_print!(
                                "Engine (re)connected with peers",
                                format!("index={}: peers_count={}", engine_index, all_peers.len())
                            );
                            if all_peers.len() == peers_list.len() {
                                break 'inner;
                            }
                        }
                    }
                }
            }
        }));

        // ~~~~~~~~~~~ spawn signing finished notification task ~~~~~~~~~~~
        let bitcoin_server_url = self.bitcoin_server_url.clone();
        let jwt_secret_path = self.jwt_secret_path.clone();
        let rx_sender = self.sender.clone();
        executor.spawn(Box::pin(async move {
            // create a btc client
            let jwt_secret = get_or_create_jwt_secret_from_path(&jwt_secret_path).unwrap();
            let mut btc_server_client = BtcServerExtendedClient::new(
                format!("http://{}", bitcoin_server_url),
                Some(jwt_secret),
            )
            .await
            .unwrap();
            loop {
                // get all session ids
                let session_ids = btc_server_client
                    .get_session_ids(GetSessionIdsRequest { max_results: 10 })
                    .await
                    .ok()
                    .map(|res| res.data)
                    .unwrap_or_default();

                // for each session get the signing status and send the response
                for session_id in session_ids.into_iter() {
                    match btc_server_client
                        .get_signing_status(GetSigningStatusRequest {
                            signing_session_id: session_id.clone(),
                        })
                        .await
                    {
                        Ok(status) => {
                            it_info_print!(
                                "Signing status fetched for index and session id",
                                engine_index,
                                session_id
                            );
                            let s = SigningStatus::try_from(status.status).ok();
                            if let Some(status) = s {
                                match rx_sender.send(Notifications::SigningStatusReport((
                                    engine_index,
                                    session_id,
                                    status,
                                ))) {
                                    Ok(_) => {}
                                    // all receivers have been dropped temporarily here. Just sleep
                                    // and await new ones to be created
                                    Err(_) => {
                                        tokio::time::sleep(Duration::from_secs(1)).await;
                                        continue;
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            it_warn_print!(
                                "Error getting signing status for index and session id ...",
                                engine_index,
                                session_id
                            );
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }));

        Ok(())
    }
}

pub fn is_dkg_ready(federation_memebers: &HashMap<u16, FederationMemberTestConfig>) -> bool {
    !federation_memebers.iter().any(|(_, member)| !member.is_dkg_ready())
}

pub async fn create_poa_federation_members(
    global_context: Arc<GlobalContext>,
    btc_servers: Option<&Vec<SpawnedBtcServer>>,
) -> (
    HashMap<u16, FederationMemberTestConfig>,
    tokio::sync::broadcast::Sender<Notifications>,
    Vec<PublicKey>,
) {
    let (tx, _rx) = tokio::sync::broadcast::channel::<Notifications>(100);

    let mut fed_members: HashMap<u16, FederationMemberTestConfig> = HashMap::new();
    let mut members_keypairs: Vec<(SecretKey, PublicKey)> = vec![];

    let mut last_rpc_port = global_context.last_poa_node_rpc_port.lock().await;
    let p = last_rpc_port.clone();
    let rpc_port_base: u16 = p + 1;
    *last_rpc_port = p + 10 + global_context.instances;
    drop(last_rpc_port);

    let mut last_discovery_port = global_context.last_poa_node_discovery_port.lock().await;
    let p = last_discovery_port.clone();
    let discovery_port_base: u16 = p + 1;
    *last_discovery_port = p + 10 + global_context.instances;
    drop(last_discovery_port);

    for _ in 0..global_context.instances {
        let secret_key = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let pk = secret_key.public_key(SECP256K1);

        members_keypairs.push((secret_key, pk));
    }
    let authorities = members_keypairs.iter().map(|(_, pk)| pk.clone()).collect::<Vec<_>>();
    let member_peer_id = pk2id(&members_keypairs[0].1);

    for member_index in 0..global_context.instances {
        let port = btc_servers
            .and_then(|servers| servers.iter().nth(member_index as usize).map(|val| val.port))
            .unwrap();

        let (test_signal_tx, _test_signal_rx) = channel::<TestSignal>(10);
        let (_finished_signing_tx, _finished_signing_rx) = channel::<TestSignal>(10);
        let fed_member_config = FederationMemberTestConfig::new(
            member_index,
            members_keypairs.to_vec().get(member_index as usize).unwrap().0,
            authorities.clone(),
            tx.clone(),
            global_context.bitcoind_url.clone(),
            global_context.bitcoind_user.clone(),
            global_context.bitcoind_pass.clone(),
            format!("localhost:{}", port),
            global_context.jwt_dir.clone(),
            global_context.min_signers,
            global_context.max_signers,
            member_peer_id,
            rpc_port_base,
            discovery_port_base,
            test_signal_tx,
        )
        .await;
        fed_members.insert(member_index, fed_member_config);
    }

    // now create the edh
    let extra_data_header = ExtraDataHeader::new(
        EXTRA_HEADER_VERSION,
        None,
        Some(authorities.clone()),
        None,
        None,
        // to make sure they're not identical, hash random data
        BlockHash::hash(&[1]),
        sha256::Hash::hash(&[2]),
    );

    // now insert peers and edh into each federation member
    for member_index in 0..global_context.instances {
        let peer_members = fed_members
            .iter()
            .filter_map(
                |(index, &ref fed_mem)| {
                    if *index != member_index {
                        Some(fed_mem.clone())
                    } else {
                        None
                    }
                },
            )
            .collect::<Vec<_>>();

        if let Some(fed_member) = fed_members.get_mut(&member_index) {
            fed_member.insert_peers_list(peer_members);
            fed_member.insert_edh(extra_data_header.clone());
        };
    }

    (fed_members, tx, authorities)
}

#[cfg(test)]
mod tests {
    

    use askama::Template;
    use bitcoin::BlockHash;
    use bitcoin::hashes::{sha256, Hash};

    use super::*;

    #[test]
    fn test_edh_template() {
        let secp: secp256k1::Secp256k1<secp256k1::All> = secp256k1::Secp256k1::new();
        let secret_key1 = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let pk1 = secp256k1::PublicKey::from_secret_key(&secp, &secret_key1);
        let secret_key2 = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let pk2 = secp256k1::PublicKey::from_secret_key(&secp, &secret_key2);

        let extra_data_header = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            None,
            Some(vec![pk1, pk2]),
            None,
            None,
            BlockHash::all_zeros(),
            sha256::Hash::all_zeros(),
        );
        let edh = hex::encode(extra_data_header.serialize());
        let botanix_testnet_config_genesis = BotanixTestnetGenesisConfig { edh: &edh };
        let rendered_json = botanix_testnet_config_genesis.render().unwrap();
        let json = serde_json::to_string_pretty(&rendered_json).unwrap();
        println!("Rendered botanix testnet configuration {json:?}");
        assert!(json.len() > 0);
    }
}
