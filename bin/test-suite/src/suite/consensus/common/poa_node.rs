use super::{
    botanix_client::BotanixEthClient, btc_server::SpawnedBtcServerProcess, kill_process_at_port,
};
use crate::{
    context::GlobalContext,
    it_error_print, it_info_print, it_warn_print,
    suite::consensus::common::{spawn_child_process, MINTING_CONTRACT_BYTECODE},
};
use anyhow::Context;
use askama::Template;
use bitcoin::hashes::Hash;
use btcserverlib::extended_client::BtcServerExtendedClient;
use client::{Empty, GetSessionIdsRequest, GetSigningStatusRequest, SigningStatus};
use ethers::providers::PeerInfo;
use reth::{
    args::{FedMemberPubKey, FederationTomlConfig},
    consensus_common::utils::unix_timestamp,
};
use reth_chainspec::{create_botanix_config_with_genesis, BOTANIX_TESTNET};
use reth_network_peers::pk2id;
use reth_primitives::{
    extra_data_header::{ExtraDataHeader, CHAIN_VERSION, EXTRA_HEADER_VERSION},
    public_key_to_address, Address,
};
use reth_provider::CanonStateNotification;
use reth_rpc_types::PeerId;
use secp256k1::{PublicKey, SecretKey, SECP256K1};
use std::{
    collections::HashMap,
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    process::Child,
    sync::broadcast::{channel, Sender},
};
use url::Url;

pub const RPC_PORT_BASE: u16 = 8545;
pub const DISCOVERY_PORT_BASE: u16 = 30330;
pub const ABCI_PORT_BASE: u16 = 26658;

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
    GetAllPeers(tokio::sync::broadcast::Sender<Vec<PeerInfo>>),
}

#[derive(Debug)]
pub struct SpawnedPoaServerProcess {
    pub rpc_port: u16,
    pub discovery_port: u16,
    pub child_process: Child,
}

impl SpawnedPoaServerProcess {
    pub async fn destroy_all_async(&mut self) {
        // kill the process
        let _ = self.child_process.kill().await;
        // additionally make sure all ports used are freed
        kill_process_at_port(self.discovery_port);
        kill_process_at_port(self.rpc_port);
    }

    pub async fn destroy_all_sync(&mut self) {
        // kill the process
        let pid = self.child_process.id().expect("Expected a process id");
        let _ = std::process::Command::new("kill")
            .arg("-9") // Use SIGKILL for immediate termination
            .arg(format!("{pid}"))
            .output();
        // additionally make sure all ports used are freed
        kill_process_at_port(self.discovery_port);
        kill_process_at_port(self.rpc_port);
    }
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
    pub abci_port: u16,
    pub bitcoind_url: Url,
    pub bitcoind_username: String,
    pub bitcoind_password: String,
    pub bitcoin_server_url: String,
    pub peers_list: Vec<FederationMemberTestConfig>,
    pub sender: tokio::sync::broadcast::Sender<Notifications>,
    pub frost_min_signers: u16,
    pub frost_max_signers: u16,
    pub peer_id: PeerId,
    pub is_dkg_ready: bool,
    pub edh: Option<ExtraDataHeader>,
    pub test_signal_tx: Sender<TestSignal>,
    pub botanix_fee_recipient: String,
    pub botanix_eth_client: Option<BotanixEthClient>,
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
        frost_min_signers: u16,
        frost_max_signers: u16,
        peer_id: PeerId,
        rpc_port: u16,
        discovery_port: u16,
        abci_port: u16,
        test_signal_tx: Sender<TestSignal>,
        botanix_fee_recipient: String,
    ) -> anyhow::Result<Self> {
        Ok(Self {
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
            abci_port,
            bitcoind_url,
            bitcoind_username,
            bitcoind_password,
            bitcoin_server_url,
            peers_list: vec![],
            sender,
            frost_min_signers,
            frost_max_signers,
            peer_id,
            is_dkg_ready: false,
            edh: None,
            test_signal_tx,
            botanix_fee_recipient,
            botanix_eth_client: None,
        })
    }

    pub fn insert_peers_list(&mut self, peers: Vec<FederationMemberTestConfig>) {
        self.peers_list = peers;
    }

    pub fn peers_list(&self) -> Vec<FederationMemberTestConfig> {
        self.peers_list.clone()
    }

    pub fn insert_edh(&mut self, edh: ExtraDataHeader) {
        self.edh = Some(edh);
    }

    pub fn is_dkg_ready(&self) -> bool {
        self.is_dkg_ready
    }

    pub fn send_test_signal(&self, signal: TestSignal) {
        if let Err(e) = self.test_signal_tx.send(signal) {
            it_error_print!("Failed to send test signal: {:?}", e);
        }
    }

    pub fn spawn_service(
        &self,
        edh_authorities_list: Arc<Vec<PublicKey>>,
    ) -> anyhow::Result<SpawnedPoaServerProcess> {
        // print secret key
        it_info_print!(format!("sk: {:?}", self.secret_key));
        it_info_print!(format!("Building federation member index: {}", self.index));

        let datadir = self.temp_path.to_str().context("created temp path is unparsable")?;
        let discovery_secret_path = Path::new(&self.temp_path).join("discovery-secret");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(discovery_secret_path.clone())
            .context("discovery secret file cannot be created/opened")?;
        let _ = file
            .write_all(&self.secret_key.display_secret().to_string().as_bytes())
            .context("error writing secret key to file")?;

        // update genesis config with edh and render file
        let botanix_testnet_config_genesis = if let Some(edh) = self.edh.as_ref() {
            let edh = hex::encode(edh.serialize());
            let botanix_testnet_config_genesis = BotanixTestnetGenesisConfig { edh: &edh };
            let rendered_json = botanix_testnet_config_genesis
                .render()
                .context("error rendering botanix testnet genesis config")?;
            rendered_json
        } else {
            return Err(anyhow::anyhow!(
                "Edh data missing. Cannot create botanix testnet config genesis file"
            ));
        };

        // Need to zip together the soc address and pk
        let mut fed_member_pks = vec![];
        for peer in self.peers_list.iter() {
            let pk = FedMemberPubKey {
                key: peer.secret_key.public_key(SECP256K1).to_string(),
                socket_addr: format!("127.0.0.1:{}", peer.discovery_port),
            };
            fed_member_pks.push(pk);
        }
        // add ourselves
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

        // Need to create a federation.toml in the data dir
        let federation_config = FederationTomlConfig::new(
            edh_authorities,
            self.botanix_fee_recipient.clone(),
            String::from(MINTING_CONTRACT_BYTECODE),
        );
        it_info_print!("Federation config", federation_config);
        let federation_config_path = Path::new(datadir).join("federation.toml");
        federation_config
            .write_to_path(&federation_config_path)
            .context("Error writing federation config to path")?;

        // point to the relevant working directory
        let mut working_directory =
            std::env::current_dir().context("Error obtaining current directory")?;
        for _ in 0..2 {
            working_directory.pop();
        }
        working_directory.push("bin");
        working_directory.push("reth");

        let federation_config_path = federation_config_path.display().to_string();
        let rpc_port = self.rpc_port.to_string();
        let bitcoin_server_url = self.bitcoin_server_url.clone();
        let bitcoind_url = self.bitcoind_url.to_string();
        let bitcoind_username = self.bitcoind_username.clone();
        let bitcoind_password = self.bitcoind_password.clone();
        let frost_min_signers = self.frost_min_signers.to_string();
        let frost_max_signers = self.frost_max_signers.to_string();
        let discovery_port = self.discovery_port.to_string();
        let abci_port = self.discovery_port.to_string();

        // prepare run arguments
        let command = "cargo";
        let args = vec![
            "run",
            "--bin",
            "reth",
            "--",
            "poa",
            "--is-testnet",
            "--ntp-server",
            "time.cloudflare.com",
            "--federation-config-path",
            federation_config_path.as_str(),
            "--federation-mode",
            "--ipcdisable",
            "--datadir",
            datadir,
            "--debug.terminate",
            "--http",
            "--http.corsdomain",
            "*",
            "--http.port",
            rpc_port.as_str(),
            "--http.addr",
            "127.0.0.1",
            "--http.api",
            "eth,net,trace,txpool,web3,rpc,admin",
            "--btc-network",
            "regtest",
            "--btc-server",
            bitcoin_server_url.as_str(),
            "--bitcoind.url",
            bitcoind_url.as_str(),
            "--bitcoind.username",
            bitcoind_username.as_str(),
            "--bitcoind.password",
            bitcoind_password.as_str(),
            "--frost.min_signers",
            frost_min_signers.as_str(),
            "--frost.max_signers",
            frost_max_signers.as_str(),
            "--port",
            discovery_port.as_str(),
            "--p2p-secret-key",
            discovery_secret_path.to_str().context("discovery secret path to exist")?,
            "--abci-port",
            abci_port.as_str(),
        ];

        // use botanix chain spec
        let genesis = serde_json::from_str(&botanix_testnet_config_genesis)
            .context("Can't deserialize Botanix Testnet genesis json")?;
        let _botanix_testnet = create_botanix_config_with_genesis(
            genesis,
            BOTANIX_TESTNET.parent_confirmation_depth,
            self.botanix_fee_recipient.clone(),
        );

        Ok(SpawnedPoaServerProcess {
            child_process: spawn_child_process(command, args, working_directory)?,
            discovery_port: self.discovery_port,
            rpc_port: self.rpc_port,
        })
    }

    pub fn await_initialization(&self) -> anyhow::Result<()> {
        it_info_print!("Engine started task with index: ", self.index);
        let engine_index = self.index;
        let rx_sender = self.sender.clone();
        let bitcoin_server_url = self.bitcoin_server_url.clone();
        let peers_list = self.peers_list.clone();
        let botanix_eth_client = self
            .botanix_eth_client
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Uninitialized botanix eth client"))?;

        // ~~~~~~~~~~ spawn initial task that adds peers and awaits dkg to finish ~~~~~~~~~~~
        tokio::spawn(Box::pin(async move {
            // add the peers
            'inner: loop {
                for peer in peers_list.iter() {
                    let peer_socket = SocketAddr::new(
                        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                        peer.discovery_port,
                    );
                    let enode_url =
                        format!("enode://{}@{}", peer.peer_id.to_string(), peer_socket.to_string());
                    if let Err(_) = botanix_eth_client.add_trusted_peer(&enode_url).await {
                        it_error_print!("RPC failed to add a peer", peer.peer_id);
                    } else {
                        it_info_print!("RPC added peer", peer.peer_id);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let all_peers = botanix_eth_client.get_peers_counts().await.unwrap_or_default();
                it_info_print!(
                    "Engine connected with peers",
                    format!("index={}: peers_count={}", engine_index, all_peers.len())
                );
                if all_peers.len() == peers_list.len() {
                    break 'inner;
                }
            }

            // create a btc client
            let mut btc_server_client =
                BtcServerExtendedClient::new(format!("http://{}", bitcoin_server_url), None)
                    .await
                    .unwrap();

            // wait for the dkg to finish
            let pub_key = loop {
                match btc_server_client.get_public_key(Empty {}).await {
                    Ok(pub_key) => {
                        it_info_print!("Dkg Finished for index", engine_index);
                        break pub_key;
                    }
                    Err(_) => {
                        it_warn_print!("Dkg Pending for engine index", engine_index);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
            };

            // send a notification about a finished dkg
            let _ = rx_sender
                .send(Notifications::DkgFinished(DkgPayload {
                    engine_index,
                    ts: tokio::time::Instant::now(),
                    public_key: pub_key.publickey,
                }))
                .unwrap();
        }));

        // ~~~~~~~~~~~ spawn a task awaiting test signals from the test suite ~~~~~~~~~~~
        let mut receiver = self.test_signal_tx.subscribe();
        let peers_list = self.peers_list.clone();
        let botanix_eth_client = self
            .botanix_eth_client
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Uninitialized botanix eth client"))?;
        tokio::spawn(Box::pin(async move {
            while let Ok(test_signal) = receiver.recv().await {
                match test_signal {
                    TestSignal::DisconnectAll() => {
                        // disconnect all peers
                        'inner: loop {
                            for peer in peers_list.iter() {
                                let peer_socket = SocketAddr::new(
                                    IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                                    peer.discovery_port,
                                );
                                let enode_url = format!(
                                    "enode://{}@{}",
                                    peer.peer_id.to_string(),
                                    peer_socket.to_string()
                                );
                                if let Err(_) =
                                    botanix_eth_client.remove_trusted_peer(&enode_url).await
                                {
                                    it_error_print!("RPC failed to remove a peer", peer.peer_id);
                                } else {
                                    it_info_print!("RPC removed peer", peer.peer_id);
                                }
                                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            }
                            let all_peers =
                                botanix_eth_client.get_peers_counts().await.unwrap_or_default();
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
                                let enode_url = format!(
                                    "enode://{}@{}",
                                    peer.peer_id.to_string(),
                                    peer_socket.to_string()
                                );
                                if let Err(_) =
                                    botanix_eth_client.add_trusted_peer(&enode_url).await
                                {
                                    it_error_print!("RPC failed to re-add a peer", peer.peer_id);
                                } else {
                                    it_info_print!("RPC re-added peer", peer.peer_id);
                                }
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            let all_peers =
                                botanix_eth_client.get_peers_counts().await.unwrap_or_default();
                            it_info_print!(
                                "Engine (re)connected with peers",
                                format!("index={}: peers_count={}", engine_index, all_peers.len())
                            );
                            if all_peers.len() == peers_list.len() {
                                break 'inner;
                            }
                        }
                    }
                    TestSignal::GetAllPeers(sender) => {
                        // get all peers
                        match botanix_eth_client.get_peers_counts().await {
                            Ok(all_peers) => {
                                it_info_print!(
                                    "Engine got all peers",
                                    format!(
                                        "index={}: peers_count={}",
                                        engine_index,
                                        all_peers.len()
                                    )
                                );
                                if let Err(e) = sender.send(all_peers) {
                                    it_error_print!("Failed to send test signal: {:?}", e);
                                }
                            }
                            Err(e) => {
                                it_error_print!("Failed to get all peers", e);
                                continue;
                            }
                        }
                    }
                }
            }
        }));

        // ~~~~~~~~~~~ spawn signing finished notification task ~~~~~~~~~~~
        let bitcoin_server_url = self.bitcoin_server_url.clone();
        let rx_sender = self.sender.clone();
        tokio::spawn(Box::pin(async move {
            // create a btc client
            let mut btc_server_client =
                BtcServerExtendedClient::new(format!("http://{}", bitcoin_server_url), None)
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

pub async fn create_poa_nodes(
    global_context: Arc<GlobalContext>,
    btc_server_processes: Option<&Vec<SpawnedBtcServerProcess>>,
) -> anyhow::Result<(
    HashMap<u16, FederationMemberTestConfig>,
    tokio::sync::broadcast::Sender<Notifications>,
    Vec<PublicKey>,
)> {
    let (tx, _rx) = tokio::sync::broadcast::channel::<Notifications>(100);

    let mut poa_nodes: HashMap<u16, FederationMemberTestConfig> = HashMap::new();
    let mut members_keypairs: Vec<(SecretKey, PublicKey, PeerId, Address)> = vec![];

    for _ in 0..global_context.fed_instances {
        let secret_key = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let pk = secret_key.public_key(SECP256K1);
        let peer_id = pk2id(&pk);
        let address = public_key_to_address(pk);
        members_keypairs.push((secret_key, pk, peer_id, address));
    }
    let authorities = members_keypairs.iter().map(|(_, pk, _, _)| pk.clone()).collect::<Vec<_>>();

    for member_index in 0..global_context.fed_instances {
        let port = btc_server_processes
            .and_then(|processes| processes.iter().nth(member_index as usize).map(|val| val.port))
            .context("Btc server process port must already exist")?;

        let (member_secretkey, _, member_peerid, _) = members_keypairs
            .get(member_index as usize)
            .cloned()
            .expect("To have keypair information");

        let (test_signal_tx, _test_signal_rx) = channel::<TestSignal>(10);
        let fed_member_config = FederationMemberTestConfig::new(
            member_index,
            member_secretkey,
            authorities.clone(),
            tx.clone(),
            global_context.bitcoind_url.clone(),
            global_context.bitcoind_user.clone(),
            global_context.bitcoind_pass.clone(),
            format!("localhost:{}", port),
            global_context.min_signers,
            global_context.max_signers,
            member_peerid,
            RPC_PORT_BASE + member_index,
            DISCOVERY_PORT_BASE + member_index,
            ABCI_PORT_BASE + 10000 * member_index,
            test_signal_tx,
            global_context.botanix_fee_recipient.clone(),
        )
        .await?;
        poa_nodes.insert(member_index, fed_member_config);
    }

    // now create the edh
    let prikey = secp256k1::SecretKey::new(&mut rand::thread_rng());
    let extra_data_header = ExtraDataHeader::new(
        EXTRA_HEADER_VERSION,
        CHAIN_VERSION,
        bitcoin::hash_types::BlockHash::all_zeros(),
        secp256k1::PublicKey::from_secret_key(secp256k1::SECP256K1, &prikey),
        Address::ZERO,
    );

    // now insert peers and edh into each federation member
    for member_index in 0..global_context.fed_instances {
        let peer_members = poa_nodes
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

        if let Some(fed_member) = poa_nodes.get_mut(&member_index) {
            fed_member.insert_peers_list(peer_members);
            fed_member.insert_edh(extra_data_header.clone());
        };
    }

    Ok((poa_nodes, tx, authorities))
}

#[cfg(test)]
mod tests {

    use super::*;
    use askama::Template;

    #[test]
    fn test_edh_template() {
        let extra_data_header = ExtraDataHeader::default();
        let edh = hex::encode(extra_data_header.serialize());
        let botanix_testnet_config_genesis = BotanixTestnetGenesisConfig { edh: &edh };
        let rendered_json = botanix_testnet_config_genesis.render().unwrap();
        let json = serde_json::to_string_pretty(&rendered_json).unwrap();
        assert!(json.len() > 0);
    }
}
