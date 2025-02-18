//! This binary is meant to setup a testnet botanix federation in one command.
//! All the configs and binaries will be setup at a output location of your choice.

mod cli;
use crate::comet_node::{get_enode, TestSignal};
use anyhow::{Context, Result as AnyResult};
use clap::Parser;
use cli::Cli;
use std::{
    fs,
    path::{Path, PathBuf},
};
use test_suite::suite::consensus::common::{
    comet_node::{self, updated_genesis_file, GenesisValidator, PrivValidator},
    poa_node::ABCI_PORT_BASE,
};
use tokio::{self, sync::broadcast::channel};

async fn create_cometbft_nodes(num_nodes: u16, output_path: PathBuf) -> AnyResult<()> {
    let mut cometbft_nodes = Vec::new();
    let cometbft_path = output_path.join("cometbft");
    // Create the output directory
    std::fs::create_dir_all(&cometbft_path)?;
    // Create the nodes
    for i in 0..num_nodes {
        let cometbft_proxy_app_port = ABCI_PORT_BASE + 1000 * i;
        let cometbft_rpc_app_port = cometbft_proxy_app_port - 1;
        let cometbft_p2p_app_port = cometbft_rpc_app_port - 1;

        let node_path = cometbft_path.join(format!("node-{}", i));
        std::fs::create_dir_all(&node_path)?;
        let (exit_status, stdout, stderr) = comet_node::init_cometbft_node(i, &node_path).await?;
        if !exit_status.success() {
            return Err(anyhow::anyhow!(
                "CometBFT node failed to initialize: {:?} {:?}",
                exit_status,
                stderr
            ));
        }
        // read priv_validator_key.json file
        let priv_validator_key_file =
            Path::new(&node_path).join("config").join("priv_validator_key.json");
        let validator =
            serde_json::from_str::<PrivValidator>(&fs::read_to_string(priv_validator_key_file)?)
                .context("Error reading priv_validator_key.json file")?;

        // get enode
        let (exit_status, stdout, stderr) =
            get_enode(i, &node_path).await.context("Error getting enode")?;
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
        let output_parts = stdout.split("\n").filter(|x| !x.is_empty()).collect::<Vec<&str>>();
        let enode = output_parts[output_parts.len() - 1].trim().to_string();
        tracing::info!("CometBFT enode: {:?}", enode);

        // prepare test signal
        let (test_signal_tx, _test_signal_rx) = channel::<TestSignal>(10);

        // create the cometbft node
        let cometbft_node = comet_node::CometBftNodeConfig::new(
            i,
            validator,
            enode,
            cometbft_proxy_app_port,
            cometbft_rpc_app_port,
            cometbft_p2p_app_port,
            test_signal_tx,
            node_path,
            false,
        )
        .await?;
        cometbft_nodes.push(cometbft_node);
    }

    let genesis_validators = cometbft_nodes
        .iter()
        .map(|c| GenesisValidator::from(&c.validator))
        .collect::<Vec<GenesisValidator>>();

    // Update all the configs with the other peer's information
    for i in 0..num_nodes {
        let node_path = cometbft_path.join(format!("node-{}", i));
        let cometbft_node = cometbft_nodes[i as usize].clone();
        updated_genesis_file(&node_path, genesis_validators.clone())
            .expect("Error updating genesis file");
        comet_node::update_config_toml(&cometbft_node).expect("Error updating config toml file");
    }

    Ok(())
}

async fn inner_main() -> AnyResult<()> {
    let cli = Cli::parse();
    // Basic sanity checks
    cli.validate()?;
    let output_path = PathBuf::from(
        cli.output_path.unwrap_or(std::env::current_dir()?.to_str().unwrap().to_string()),
    ).join("output");
    println!("Output path: {:?}", output_path);

    // Create the output directory
    std::fs::create_dir_all(&output_path)?;

    // Create the cometbft nodes
    create_cometbft_nodes(cli.num_nodes, output_path).await?;

    // Create the output directory
    Ok(())
}
#[tokio::main]
async fn main() {
    if let Err(e) = inner_main().await {
        eprintln!("ERROR: {}", e);
    }
}
