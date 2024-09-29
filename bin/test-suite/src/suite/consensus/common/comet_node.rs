use super::{botanix_client::BotanixEthClient, kill_process_at_port, poa_node::ABCI_PORT_BASE};
use crate::{context::GlobalContext, suite::consensus::common::spawn_child_process};
use anyhow::Context;
use askama::Template;
use reth::consensus_common::utils::unix_timestamp;
use reth_network_peers::pk2id;
use reth_primitives::{alloy_primitives::Keccak256, public_key_to_address, Address};
use reth_rpc_types::PeerId;
use secp256k1::{PublicKey, SecretKey, SECP256K1};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    process::Child,
    sync::broadcast::{channel, Sender},
};

fn generate_secrets() -> (SecretKey, PublicKey, PeerId, Address, String) {
    let secret_key = secp256k1::SecretKey::new(&mut rand::thread_rng());
    let public_key = secret_key.public_key(SECP256K1);
    let peer_id = pk2id(&public_key);
    let address = public_key_to_address(public_key);
    let extended_key = [public_key.serialize_uncompressed()].concat();
    let extended_key_base64 = base64::encode(extended_key);
    (secret_key, public_key, peer_id, address, extended_key_base64)
}

fn produce_base64_encoded_data(secret_key: &SecretKey) -> (String, String, String) {
    let public_key = secret_key.public_key(SECP256K1).serialize();

    // Calculate the address (first 20 bytes of the SHA-256 hash of the public key)
    let mut hasher = Keccak256::new();
    hasher.update(&public_key);
    let address = &hasher.finalize()[..20]; // Take the first 20 bytes

    // Encode the public and private keys in Base64 format (as expected by Tendermint)
    let priv_key_base64 = base64::encode(secret_key.as_ref());
    let pub_key_base64 = base64::encode(&public_key); // Base64 encoding of the public key
    let address_hex = hex::encode(address);

    (priv_key_base64, pub_key_base64, address_hex)
}

// fn generate_extended_priv_key_ed25519() -> String {
//     let mut csprng = OsRng {};
//     let signing_key = ed25519_dalek::SigningKey::generate(&mut csprng);

//     // Extract the 32-byte private key
//     let secret_key_bytes = signing_key.to_bytes();

//     // Extract the 32-byte public key
//     let public_key_bytes = signing_key.verifying_key().to_bytes();

//     // Combine the private key and public key (64 bytes total)
//     let extended_key = [secret_key_bytes.as_slice(), public_key_bytes.as_slice()].concat();

//     // Encode the combined key in Base64
//     let extended_key_base64 = base64::encode(extended_key);

//     extended_key_base64
// }

#[derive(Clone, Debug)]
pub enum Notifications {}

#[derive(Clone, Debug)]
pub enum TestSignal {
    DisconnectAll(),
    ReconnectAll(),
}

pub trait TemplateWriter {
    fn write_to_file(&self, path: impl AsRef<Path> + Send, filename: &str) -> anyhow::Result<()>
    where
        Self: askama::Template + Serialize,
    {
        let rendered_template = self.render().context("Failed to render dynamic template")?;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path.as_ref().to_path_buf().join(filename))
            .context("Failed to create/open a file")?;

        let res = file
            .write_all(rendered_template.as_bytes())
            .context("Failed to write contents to a file");
        res
    }
}

// =============================== TEMPLATES =========================== //
/// Templates
#[derive(Template, Clone, Debug, Serialize)]
#[template(path = "cometbft/config/node_key.json", ext = "json", escape = "none")]
struct NodeKeyConfigTemplate<'a> {
    validator_node_key: &'a str,
}

impl TemplateWriter for NodeKeyConfigTemplate<'_> {}

#[derive(Template, Clone, Debug, Serialize)]
#[template(path = "cometbft/config/priv_validator_key.json", ext = "json", escape = "none")]
struct PrivValidatorKeyConfigTemplate<'a> {
    validator_address: &'a str,
    validator_pub_key: &'a str,
    validator_priv_key: &'a str,
}

impl TemplateWriter for PrivValidatorKeyConfigTemplate<'_> {}

#[derive(Template, Clone, Debug, Serialize)]
#[template(path = "cometbft/data/priv_validator_state.json", ext = "json", escape = "none")]
struct PrivValidatorStateTemplate<'a> {
    height: &'a str,
}

impl TemplateWriter for PrivValidatorStateTemplate<'_> {}

#[derive(Template, Clone, Debug, Serialize)]
#[template(path = "cometbft/config/config.toml", ext = "toml", escape = "none")]
struct ValidatorConfigTemplate<'a> {
    rpc_app_port: u16,
    proxy_app_port: u16,
    persistent_peers: &'a str,
}

impl TemplateWriter for ValidatorConfigTemplate<'_> {}

#[derive(Clone, Debug, Template, Serialize)]
#[template(path = "cometbft/config/genesis.txt", ext = "json", escape = "none")]
pub struct GenesisTemplate {
    validators: Vec<Validator>,
}

impl TemplateWriter for GenesisTemplate {}

#[derive(Clone, Debug, Serialize)]
pub struct Validator {
    address: String,
    pub_key_type: String,
    pub_key_value: String,
    power: String,
    name: String,
}
// ================================================================= //

#[derive(Debug)]
pub struct SpawnedCometBftProcess {
    pub cometbft_proxy_app_port: u16,
    pub cometbft_rpc_app_port: u16,
    pub child_process: Child,
}

impl SpawnedCometBftProcess {
    pub async fn destroy_all_async(&mut self) {
        // kill the process
        let _ = self.child_process.kill().await;
        // additionally make sure all ports used are freed
        kill_process_at_port(self.cometbft_proxy_app_port);
        kill_process_at_port(self.cometbft_rpc_app_port);
    }

    pub async fn destroy_all_sync(&mut self) {
        // kill the process
        let pid = self.child_process.id().expect("Expected a process id");
        let _ = std::process::Command::new("kill")
            .arg("-9") // Use SIGKILL for immediate termination
            .arg(format!("{pid}"))
            .output();
        // additionally make sure all ports used are freed
        kill_process_at_port(self.cometbft_proxy_app_port);
        kill_process_at_port(self.cometbft_rpc_app_port);
    }
}

#[derive(Clone, Debug)]
pub struct CometBftNodeConfig {
    pub index: u16,
    pub temp_path: PathBuf,
    pub secret_key: SecretKey,
    pub cometbft_proxy_app_port: u16,
    pub cometbft_rpc_app_port: u16,
    pub peers_list: Vec<CometBftNodeConfig>,
    pub peer_id: PeerId,
    pub botanix_eth_client: Option<BotanixEthClient>,
    pub test_signal_tx: Sender<TestSignal>,
}

impl CometBftNodeConfig {
    pub async fn new(
        index: u16,
        secret_key: SecretKey,
        peer_id: PeerId,
        cometbft_proxy_app_port: u16,
        cometbft_rpc_app_port: u16,
        test_signal_tx: Sender<TestSignal>,
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
            peers_list: vec![],
            peer_id,
            botanix_eth_client: None,
            cometbft_proxy_app_port,
            cometbft_rpc_app_port,
            test_signal_tx,
        })
    }

    pub fn insert_peers_list(&mut self, peers: Vec<CometBftNodeConfig>) {
        self.peers_list = peers;
    }

    pub fn peers_list(&self) -> Vec<CometBftNodeConfig> {
        self.peers_list.clone()
    }

    pub fn spawn_service(&self) -> anyhow::Result<SpawnedCometBftProcess> {
        // point to the relevant working directory
        let mut working_directory =
            std::env::current_dir().context("Error obtaining current directory")?;
        for _ in 0..2 {
            working_directory.pop();
        }
        working_directory.push("cometbft");

        // prepare run arguments
        let home_path = self.temp_path.to_path_buf();
        let home_path_str = home_path.display().to_string();
        let command = "cometbft";
        let args = vec!["start", "--home", &home_path_str];

        Ok(SpawnedCometBftProcess {
            child_process: spawn_child_process(command, args, working_directory)?,
            cometbft_proxy_app_port: self.cometbft_proxy_app_port,
            cometbft_rpc_app_port: self.cometbft_rpc_app_port,
        })
    }
}

impl CometBftNodeConfig {
    pub fn await_initialization(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

pub async fn create_cometbft_nodes(
    global_context: Arc<GlobalContext>,
) -> anyhow::Result<(
    HashMap<u16, CometBftNodeConfig>,
    tokio::sync::broadcast::Sender<Notifications>,
    Vec<PublicKey>,
)> {
    let (tx, _rx) = tokio::sync::broadcast::channel::<Notifications>(100);

    let mut cometbft_nodes: HashMap<u16, CometBftNodeConfig> = HashMap::new();
    let mut members_signing_keypairs: Vec<(SecretKey, PublicKey, PeerId, Address, String)> = vec![];
    let mut members_cometbft_ports: Vec<(u16, u16)> = vec![];

    for member_index in 0..global_context.fed_instances {
        members_signing_keypairs.push(generate_secrets());
        let cometbft_proxy_app_port = ABCI_PORT_BASE + 10000 * member_index;
        let cometbft_rpc_app_port = cometbft_proxy_app_port - 1;
        members_cometbft_ports.push((cometbft_proxy_app_port, cometbft_rpc_app_port));
    }
    let authorities =
        members_signing_keypairs.iter().map(|(_, pk, _, _, _)| pk.clone()).collect::<Vec<_>>();

    for member_index in 0..global_context.fed_instances {
        let (test_signal_tx, _test_signal_rx) = channel::<TestSignal>(10);
        let cometbft_proxy_app_port = ABCI_PORT_BASE + 10000 * member_index;
        let cometbft_rpc_app_port = cometbft_proxy_app_port - 1;
        let (
            cometbft_signing_secretkey,
            _cometbft_signing_pubkey,
            cometbft_signing_peerid,
            _cometbft_signing_address,
            cometbft_signing_extended_key,
        ) = members_signing_keypairs
            .get(member_index as usize)
            .cloned()
            .expect("To have keypair information");
        let cometbft_node = CometBftNodeConfig::new(
            member_index,
            cometbft_signing_secretkey,
            cometbft_signing_peerid,
            cometbft_proxy_app_port,
            cometbft_rpc_app_port,
            test_signal_tx,
        )
        .await?;

        // ~~~~~~~~~~~~~~~~~~ write config.toml file ~~~~~~~~~~~~~~~~~
        let cometbft_config_file = Path::new(&cometbft_node.temp_path).join("config");
        fs::create_dir_all(&cometbft_config_file)?;
        let counterpeer_enodes = members_signing_keypairs
            .iter()
            .enumerate()
            .filter_map(|(index, peer_data)| {
                let (_cometbft_proxy_app_port, cometbft_rpc_app_port) =
                    members_cometbft_ports.get(index).cloned().expect("to have cometbft ports");
                if index as u16 != member_index {
                    let enode_url =
                        format!("enode://{}@{}", peer_data.2.to_string(), cometbft_rpc_app_port);
                    return Some(enode_url);
                } else {
                    return None;
                }
            })
            .collect::<Vec<String>>();
        ValidatorConfigTemplate {
            proxy_app_port: cometbft_node.cometbft_proxy_app_port,
            rpc_app_port: cometbft_node.cometbft_rpc_app_port,
            persistent_peers: &counterpeer_enodes.join(","),
        }
        .write_to_file(&cometbft_config_file, "config.toml")
        .context("Error writing cometbft_config_file to path")?;

        // ~~~~~~~~~~~~~~~~~~ write priv_validator_state json file ~~~~~~~~~~~~~~~~~~
        let cometbft_priv_validator_state_file = Path::new(&cometbft_node.temp_path).join("data");
        fs::create_dir_all(&cometbft_priv_validator_state_file)?;
        PrivValidatorStateTemplate { height: "0" }
            .write_to_file(&cometbft_priv_validator_state_file, "priv_validator_state.json")
            .context("Error writing cometbft_priv_validator_state_file to path")?;

        // ~~~~~~~~~~~~~~~~~~ write node_key json file ~~~~~~~~~~~~~~~~~~
        let cometbft_nodekey_file = Path::new(&cometbft_node.temp_path).join("config");
        fs::create_dir_all(&cometbft_nodekey_file)?;
        NodeKeyConfigTemplate { validator_node_key: &cometbft_signing_extended_key }
            .write_to_file(&cometbft_nodekey_file, "node_key.json")
            .context("Error writing node_key to path")?;

        // ~~~~~~~~~~~~~~~~~~ write priv_validator_key json file ~~~~~~~~~~~~~~~~~~
        let (
            cometbft_signing_secretkey,
            _cometbft_signing_pubkey,
            _cometbft_peerid,
            _cometbft_signing_address,
            _cometbft_signing_extended_key,
        ) = members_signing_keypairs
            .get(member_index as usize)
            .cloned()
            .expect("To have keypair information");
        let cometbft_priv_validator_key_file = Path::new(&cometbft_node.temp_path).join("config");
        fs::create_dir_all(&cometbft_priv_validator_key_file)?;
        let (priv_key_base64, pub_key_base64, address_hex) =
            produce_base64_encoded_data(&cometbft_signing_secretkey);
        PrivValidatorKeyConfigTemplate {
            validator_address: &address_hex,
            validator_priv_key: &priv_key_base64,
            validator_pub_key: &pub_key_base64,
        }
        .write_to_file(&cometbft_priv_validator_key_file, "priv_validator_key.json")
        .context("Error writing node_key to path")?;
        // ~~~~~~~~~~~~~~~~~~ write cometbft_genesis_file json file ~~~~~~~~~~~~~~~~~~
        let validators = members_signing_keypairs
            .iter()
            .map(|(secret_key, _public_key, _peer_id, _address, _extended_key)| {
                let (_priv_key_base64, pub_key_base64, address_hex) =
                    produce_base64_encoded_data(secret_key);
                Validator {
                    address: address_hex,
                    name: "".to_string(),
                    power: "10".to_string(),
                    pub_key_type: "tendermint/PubKeySecp256k1".to_string(),
                    pub_key_value: pub_key_base64,
                }
            })
            .collect::<Vec<_>>();
        let cometbft_genesis_file = Path::new(&cometbft_node.temp_path).join("config");
        fs::create_dir_all(&cometbft_priv_validator_key_file)?;
        GenesisTemplate { validators }
            .write_to_file(&cometbft_genesis_file, "genesis.json")
            .context("Error writing cometbft_genesis_file to path")?;
        cometbft_nodes.insert(member_index, cometbft_node);
    }

    // now insert peers and edh into each federation member
    for member_index in 0..global_context.fed_instances {
        let peer_members = cometbft_nodes
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

        if let Some(fed_member) = cometbft_nodes.get_mut(&member_index) {
            fed_member.insert_peers_list(peer_members);
        };
    }

    Ok((cometbft_nodes, tx, authorities))
}

#[cfg(test)]
mod tests {

    use super::*;
    use askama::Template;
    use reth_primitives::alloy_primitives::Keccak256;

    #[test]
    fn test_config_toml_template() {
        let validator_config = ValidatorConfigTemplate {
            proxy_app_port: 26658,
            rpc_app_port: 26657,
            persistent_peers: "",
        };
        let rendered = validator_config.render().unwrap();
        let toml: toml::Value = toml::from_str(&rendered).unwrap();
        let proxy_app_port = toml.get("proxy_app").map(|val| val.as_str()).flatten();
        assert_eq!(proxy_app_port, Some("tcp://127.0.0.1:26658"));
        let rpc_app_port =
            toml.get("rpc").map(|val| val.get("laddr")).flatten().map(|val| val.as_str()).flatten();
        assert_eq!(rpc_app_port, Some("tcp://0.0.0.0:26657"));
    }

    #[test]
    fn test_genesis_template() {
        let validators = vec![
            Validator {
                address: "7A58E9295D0593F6FECA345FFCC3855B75B5FA8D".to_string(),
                pub_key_type: "tendermint/PubKeySecp256k1".to_string(),
                pub_key_value: "An95QAPC0caOmiUs2D1zkcu3wmgGmOS2IpGQUinTxcl6".to_string(),
                power: "10".to_string(),
                name: "".to_string(),
            },
            Validator {
                address: "4D03752E0CDF6463E6076F7570F5F89D96D9DE8D".to_string(),
                pub_key_type: "tendermint/PubKeySecp256k1".to_string(),
                pub_key_value: "AsDmE5uZBz0+0kc3y2Af8N+gsgBuymXmi/GHW/fwyaJD".to_string(),
                power: "10".to_string(),
                name: "".to_string(),
            },
        ];

        let genesis = GenesisTemplate { validators };

        // Render the template
        let rendered = genesis.render().unwrap();
        let jsonified: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let validators = jsonified.get("validators").unwrap().clone().as_array().unwrap().clone();
        assert!(validators.len() == validators.len());
    }

    #[test]
    fn test_nodekey_template() {
        let ret = tempfile::TempDir::new()
            .expect("tempdir is okay")
            .into_path()
            .join(format!("_{}", unix_timestamp().to_string()));
        std::fs::create_dir_all(&ret).expect("failed to create tempdir subdir");

        let (_, _, _, _, extended_key) = generate_secrets();
        let template = NodeKeyConfigTemplate { validator_node_key: &extended_key };

        // Render the template
        let rendered = template.render().unwrap();
        let jsonified: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let privkey = jsonified
            .get("priv_key")
            .unwrap()
            .as_object()
            .map(|x| x.get("value"))
            .flatten()
            .map(|x| x.as_str())
            .flatten()
            .map(|x| x.len())
            .unwrap_or_default();
        assert!(privkey > 0);
    }

    #[test]
    fn test_priv_validator_key_template() {
        let ret = tempfile::TempDir::new()
            .expect("tempdir is okay")
            .into_path()
            .join(format!("_{}", unix_timestamp().to_string()));
        std::fs::create_dir_all(&ret).expect("failed to create tempdir subdir");

        let secret_key = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let public_key = secret_key.public_key(SECP256K1).serialize();

        // Calculate the address (first 20 bytes of the SHA-256 hash of the public key)
        let mut hasher = Keccak256::new();
        hasher.update(&public_key);
        let address = &hasher.finalize()[..20]; // Take the first 20 bytes

        // Encode the public and private keys in Base64 format (as expected by Tendermint)
        let priv_key_base64 = base64::encode(secret_key.as_ref());
        let pub_key_base64 = base64::encode(&public_key); // Base64 encoding of the public key
        let address_hex = hex::encode(address);

        let template = PrivValidatorKeyConfigTemplate {
            validator_address: &address_hex,
            validator_priv_key: &priv_key_base64,
            validator_pub_key: &pub_key_base64,
        };

        // Render the template
        let rendered = template.render().unwrap();
        let jsonified: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert!(jsonified.get("pub_key").is_some());
        assert!(jsonified.get("priv_key").is_some());
        assert!(jsonified.get("address").is_some());
    }
}
