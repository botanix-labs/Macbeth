//! This binary is meant to setup a testnet botanix federation in one command.
//! All the configs and binaries will be setup at a output location of your choice.

mod cli;
use crate::comet_node::{get_enode, TestSignal};
use anyhow::{Context, Result as AnyResult};
use clap::Parser;
use cli::Cli;
use reth_node_core::{
    args::{FedMemberPubKey, FederationTomlConfig},
    primitives::Address,
};
use secp256k1::SECP256K1;
use std::collections::HashMap;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use test_suite::suite::consensus::common::comet_node::HostAndPort;
use test_suite::suite::consensus::common::{
    comet_node::{self, updated_genesis_file, GenesisValidator, PrivValidator},
    poa_node::{ABCI_PORT_BASE, DISCOVERY_PORT_BASE},
    MINTING_CONTRACT_BYTECODE,
};
use tokio::{self, sync::broadcast::channel};

async fn create_cometbft_node_configs(cli: &Cli) -> AnyResult<Vec<comet_node::CometBftNodeConfig>> {
    let mut cometbft_node_configs = Vec::new();

    // Create the nodes
    for i in 0..cli.num_nodes {
        let cometbft_path = cli.output_path.join(format!("node-{}", i + 1)).join("cometbft");

        // Create the output directory
        fs::create_dir_all(&cometbft_path)?;

        let proxy_app_address = if cli.non_docker {
            format!("127.0.0.1:{}", ABCI_PORT_BASE + 1000 * i)
        } else {
            format!("{}{}-poa-1:{}", cli.project_name_prefix, i + 1, ABCI_PORT_BASE)
        }
        .parse()
        .context("failed to parse cometbft proxy app address")?;

        let rpc_listen_address = if cli.non_docker {
            format!("127.0.0.1:{}", (ABCI_PORT_BASE + 1000 * i) - 1)
        } else {
            format!("0.0.0.0:{}", 26657)
        }
        .parse()
        .context("failed to parse cometbft rpc listen address")?;

        let p2p_listen_port = if cli.non_docker { (ABCI_PORT_BASE + 1000 * i) - 2 } else { 26656 };
        let p2p_listen_host = if cli.non_docker { "127.0.0.1" } else { "0.0.0.0" };

        let p2p_listen_address = format!("{}:{}", p2p_listen_host, p2p_listen_port)
            .parse()
            .context("failed to parse cometbft p2p listen address")?;

        let node_external_address = if cli.non_docker {
            format!("127.0.0.1:{}", p2p_listen_port)
        } else {
            format!("{}{}-cometbft-1:{}", cli.project_name_prefix, i + 1, p2p_listen_port)
        }
        .parse()
        .context("failed to parse cometbft node external address")?;

        let (exit_status, _stdout, stderr) = comet_node::init_cometbft_node(i, &cometbft_path)
            .await
            .context("failed to init cometbft node")?;

        if !exit_status.success() {
            return Err(anyhow::anyhow!(
                "CometBFT node failed to initialize: {:?} {:?}",
                exit_status,
                stderr
            ));
        }
        // read priv_validator_key.json file
        let priv_validator_key_file = cometbft_path.join("config").join("priv_validator_key.json");

        let priv_validator_key_file_str = fs::read_to_string(priv_validator_key_file)
            .context("Error reading priv_validator_key.json file")?;

        let validator = serde_json::from_str::<PrivValidator>(&priv_validator_key_file_str)
            .context("Error decoding priv_validator_key.json file")?;

        // get enode
        let (exit_status, stdout, stderr) =
            get_enode(i, &cometbft_path).await.context("Error getting enode")?;

        if !exit_status.success() {
            tracing::error!(
                "CometBFT enode failed to be obtained: {:?} {:?} {:?}",
                exit_status,
                stdout,
                stderr
            );
            return Err(anyhow::anyhow!(
                "CometBFT enode failed to be obtained: {:?} {:?}",
                exit_status,
                stderr
            ));
        }
        let output_parts = stdout.split('\n').filter(|x| !x.is_empty()).collect::<Vec<&str>>();
        let enode = output_parts[output_parts.len() - 1].trim().to_string();
        tracing::info!("CometBFT enode: {:?}", enode);

        // prepare test signal
        let (test_signal_tx, _test_signal_rx) = channel::<TestSignal>(10);

        // create the cometbft node
        let cometbft_node_config = comet_node::CometBftNodeConfig::new(
            i,
            validator,
            enode,
            proxy_app_address,
            rpc_listen_address,
            p2p_listen_address,
            test_signal_tx,
            cometbft_path,
            node_external_address,
            false,
        )
        .await?;

        cometbft_node_configs.push(cometbft_node_config);
    }

    let genesis_validators: Vec<_> =
        cometbft_node_configs.iter().map(|c| GenesisValidator::from(&c.validator)).collect();

    // Update all the configs with the other peer's information
    for i in 0..cli.num_nodes {
        let cometbft_path = cli.output_path.join(format!("node-{}", i + 1)).join("cometbft");
        let mut cometbft_config = cometbft_node_configs[i as usize].clone();

        // Update peers list
        cometbft_config.insert_peers_list(cometbft_node_configs.clone());

        updated_genesis_file(&cometbft_path, genesis_validators.clone())
            .context("failed updating genesis file")?;

        comet_node::update_config_toml(&cometbft_config)
            .context("Error updating config toml file")?;
    }

    Ok(cometbft_node_configs)
}

struct FederationMemberConfig {
    index: u16,
    secret_key: secp256k1::SecretKey,
    public_key: secp256k1::PublicKey,
    path: PathBuf,
    discovery_secret_path: PathBuf,
    jwt_secret_path: PathBuf,
    socket_address: HostAndPort,
}
fn create_poa_node_configs(cli: &Cli) -> AnyResult<Vec<FederationMemberConfig>> {
    let mut configs = Vec::new();

    for i in 0..cli.num_nodes {
        // Create config dir for the node
        let node_path = cli.output_path.join(format!("node-{}", i + 1)).join("poa");
        fs::create_dir_all(&node_path)?;

        // Create the secret key
        let sk = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let pk = sk.public_key(SECP256K1);

        // Write the discovery secret key
        let discovery_secret_path = node_path.join("discovery-secret");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(discovery_secret_path.clone())
            .context("discovery secret file cannot be created/opened")?;
        file.write_all(sk.display_secret().to_string().as_bytes())
            .context("error writing secret key to file")?;

        // Lastly we need to create the jwt secret key
        let jwt_secret_path = node_path.join("bjwt.hex");
        let jwt_sk = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&jwt_secret_path)
            .context("jwt secret file cannot be created/opened")?;
        file.write_all(jwt_sk.display_secret().to_string().as_bytes())
            .context("error writing jwt secret key to file")?;

        let socket_address = if cli.non_docker {
            format!("127.0.0.1:{}", DISCOVERY_PORT_BASE + 1000 * i)
        } else {
            format!("{}{}-poa-1:{}", cli.project_name_prefix, i + 1, DISCOVERY_PORT_BASE)
        }
        .parse()?;

        let config = FederationMemberConfig {
            index: i,
            secret_key: sk,
            public_key: pk,
            path: node_path,
            discovery_secret_path,
            jwt_secret_path,
            socket_address,
        };

        configs.push(config);
    }

    Ok(configs)
}

fn create_federation_config(members: &[FederationMemberConfig]) -> AnyResult<FederationTomlConfig> {
    let random_fee_recipient = Address::random();
    let random_lst_fee_receiver = Address::random();

    let fed_pks = members
        .iter()
        .map(|member| FedMemberPubKey {
            key: member.public_key.to_string(),
            socket_addr: member.socket_address.to_string(),
        })
        .collect();

    let config = FederationTomlConfig {
        federation_member_public_key: fed_pks,
        botanix_fee_recipient: random_fee_recipient.to_string(),
        minting_contract_bytecode: String::from(MINTING_CONTRACT_BYTECODE),
        lst_fee_receiver: random_lst_fee_receiver.to_string(),
    };

    for member in members {
        let federation_config_path = member.path.join("federation.toml");

        config.write_to_path(&federation_config_path)?;
    }

    Ok(config)
}

fn create_docker_compose_dot_env_file(
    cli: &Cli,
    comet_configs: &[comet_node::CometBftNodeConfig],
) -> AnyResult<()> {
    for config in comet_configs {
        let project_name = format!("{}{}", cli.project_name_prefix, config.index + 1);

        let node_path = cli.output_path.join(format!("node-{}", config.index + 1));

        let env_config = HashMap::from([
            ("COMPOSE_PROJECT_NAME", project_name),
            ("BLOCK_FEE_RECIPIENT_ADDRESS", cli.block_fee_recipient.clone()),
            ("BOTANIX_HOME", node_path.to_str().unwrap().to_string()),
            ("NTP_SERVER_URL", "time.cloudflare.com".to_string()),
            ("BITCOIND_NETWORK", "regtest".to_string()),
            ("BITCOIND_URL", "http://bitcoin-core:8332".to_string()),
            ("BITCOIND_USER", "foo".to_string()),
            ("BITCOIND_PASSWORD", "bar".to_string()),
            ("FROST_MIN_SIGNERS", cli.multisig_min_signers().to_string()),
            ("FROST_MAX_SIGNERS", cli.multisig_max_signers().to_string()),
            ("POA_RPC_PORT", (8545 + config.index * 100).to_string()),
            ("POA_WS_PORT", (8546 + config.index * 100).to_string()),
            ("POA_METRICS_PORT", (9001 + config.index * 100).to_string()),
            ("BTC_SERVER_PORT", (8080 + config.index * 100).to_string()),
            ("BTC_SERVER_ID", config.index.to_string()),
            ("COMET_BFT_RPC_PORT", (26657 + config.index * 100).to_string()),
            ("COMET_BFT_METRICS_PORT", (26658 + config.index * 100).to_string()),
        ]);

        let env_file = node_path.join(".env");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&env_file)
            .context("Error creating .env file")?;

        // Write CometBFT node configs
        for (key, value) in &env_config {
            writeln!(file, "{}={}", key, value).context("Error writing to .env file")?;
        }
    }

    Ok(())
}

fn copy_poa_configs_to_btc_server(
    poa_configs: &[FederationMemberConfig],
    output_path: &Path,
) -> AnyResult<()> {
    for config in poa_configs {
        let btc_server_path =
            output_path.join(format!("node-{}", config.index + 1)).join("btc_server");

        // Create the btc server directory
        fs::create_dir_all(&btc_server_path)?;

        let files = ["discovery-secret", "bjwt.hex", "federation.toml"];

        for file in files {
            let source_path = config.path.join(file);
            let destination_path = btc_server_path.join(file);

            fs::copy(source_path, destination_path)
                .with_context(|| format!("Error copying {} to btc server", file))?;
        }

        // Copy config.toml file
        let current_crate_dir = env!("CARGO_MANIFEST_DIR");
        let workspace_root = Path::new(current_crate_dir).parent().unwrap().parent().unwrap(); // Go up from bin/botanix-up to workspace root
        let btc_server_config_toml =
            workspace_root.join("bin").join("btc-server").join("config.toml");
        let config_toml_destination = btc_server_path.join("config.toml");

        fs::copy(&btc_server_config_toml, &config_toml_destination).with_context(|| {
            format!(
                "Error copying config.toml from {} to btc server",
                btc_server_config_toml.display()
            )
        })?;
    }

    Ok(())
}

async fn inner_main() -> AnyResult<()> {
    let cli = Cli::parse();

    // Basic sanity checks
    cli.validate()?;

    println!("Output path: {:?}", &cli.output_path);

    // Create the output directory
    fs::create_dir_all(&cli.output_path)?;

    let comet_configs = create_cometbft_node_configs(&cli)
        .await
        .with_context(|| "failed to create cometbft configs")?;

    let poa_configs = create_poa_node_configs(&cli).context("create poa node configs")?;

    create_federation_config(&poa_configs).context("creating federation config")?;

    copy_poa_configs_to_btc_server(&poa_configs, &cli.output_path)
        .context("copying poa configs to btc server")?;

    if !cli.non_docker {
        // Create the docker-compose.yml file
        create_docker_compose_dot_env_file(&cli, &comet_configs)
            .context("creating docker compose .env file")?;
    }

    // Create the output directory
    Ok(())
}
#[tokio::main]
async fn main() {
    if let Err(e) = inner_main().await {
        eprintln!("ERROR: {:?}", e);
    }
}
