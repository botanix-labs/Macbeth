# Botanix Local Development Environment

A simple CLI tool to manage a local Botanix development environment with docker-compose that sets up a simple Botanix Federation of multiple nodes, Bitcoin regtest, and CometBFT consensus.

## Prerequisites

Before you begin, ensure you have the following installed on your system:

### Required Software

1. **Docker & Docker Compose**
   - Docker Engine 20.10+ or Docker Desktop
   - Docker Compose V2 (comes with Docker Desktop)
   - Verify installation:
     ```bash
     docker --version
     docker compose version
     ```
   - Installation guides:
     - [Docker Desktop for Mac](https://docs.docker.com/desktop/install/mac-install/)
     - [Docker Desktop for Windows](https://docs.docker.com/desktop/install/windows-install/)
     - [Docker Engine for Linux](https://docs.docker.com/engine/install/)

2. **Rust & Cargo**
   - Rust 1.70+ (for building Botanix components)
   - Verify installation:
     ```bash
     rustc --version
     cargo --version
     ```
   - Install via [rustup](https://rustup.rs/):
     ```bash
     curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
     ```

3. **CometBFT**
   - CometBFT v1.0.0+
   - Required for node ID extraction during setup
   - Verify installation:
     ```bash
     cometbft version
     ```
   - Installation:
     ```bash
     # macOS (via Homebrew)
     brew install cometbft

     # Linux (download binary)
     wget https://github.com/cometbft/cometbft/releases/download/v1.0.1/cometbft_1.0.1_linux_amd64.tar.gz
     tar -xzf cometbft_1.0.1_linux_amd64.tar.gz
     sudo mv cometbft /usr/local/bin/
     ```


### System Requirements

- **RAM**: Minimum 8GB (16GB recommended for 3+ nodes)
- **Disk Space**: At least 10GB free space
- **CPU**: 4+ cores recommended
- **OS**: macOS, Linux, or Windows with WSL2

### Permissions

- Docker must be running and accessible without sudo (add user to `docker` group on Linux)
- Write permissions in the project directory for config generation

### Network Requirements

- Ports available (default configuration):
  - `18443`, `38332`, `28332` - Bitcoin Core
  - `8545-8945` range - POA RPC (100 per node)
  - `8546-8946` range - POA WebSocket (100 per node)
  - `8080-8380` range - BTC Server (100 per node)
  - `26657-26957` range - CometBFT RPC (100 per node)

## Quick Start

### 1. Setup Configuration

Copy the example environment file and customize it:

```bash
cp .env.example .env
```

Edit `.env` with your preferred settings:

```bash
# Key settings you might want to change
NUM_NODES=3                    # Number of Federation nodes
MIN_SIGNERS=2                  # Minimum signers for FROST
MAX_SIGNERS=3                  # Maximum signers for FROST
OUTPUT_PATH=docker-local       # Where to generate configs
```

### 2. Start the Environment

```bash
chmod +x botanix-local.sh
./botanix-local.sh start
```

This single command will:
- Generate all node configurations
- Create Docker network
- Start all services (Bitcoin, btc-server, poa, cometbft)
- Initialize Bitcoin regtest with 1000 blocks

### 3. Check Status

```bash
./botanix-local.sh services
```

### 4. Stop When Done

```bash
./botanix-local.sh stop
```

## Commands

### Core Commands

| Command | Description |
|---------|-------------|
| `start` | Setup and start all services |
| `stop` | Stop all running services |
| `restart` | Restart all services |
| `clean` | Remove all containers, volumes, and generated config and data files |

### Monitoring Commands

| Command | Description |
|---------|-------------|
| `services` | Show status of all services |
| `logs` | Show logs for all services |
| `logs <service>` | Show logs for specific service (e.g., `logs poa-1`) |

### Utility Commands

| Command | Description |
|---------|-------------|
| `exec <service> [cmd]` | Execute command in a service container |
| `bitcoin <args>` | Execute bitcoin-cli commands |
| `help` | Show help message |

## Usage Examples

### Get Started

```bash
# First time setup
cp .env.example .env
vim .env  # Customize if needed

# Start services
./botanix-local.sh start
```

### Viewing Logs

```bash
# All services
./botanix-local.sh logs

# Specific service
./botanix-local.sh logs poa-1
./botanix-local.sh logs btc-server-1
./botanix-local.sh logs cometbft-1
./botanix-local.sh logs bitcoin-core
```

### Bitcoin Operations

```bash
# Get block count
./botanix-local.sh bitcoin getblockcount

# Get wallet balance
./botanix-local.sh bitcoin -rpcwallet=testwallet getbalance

# Get blockchain info
./botanix-local.sh bitcoin getblockchaininfo

# Generate more blocks
./botanix-local.sh bitcoin -rpcwallet=testwallet generatetoaddress 10 <address>
```

### Container Access

```bash
# Open shell in a service
./botanix-local.sh exec poa-1

# Run specific command
./botanix-local.sh exec poa-1 ls -la /reth/botanix_testnet/config
```

### Complete Cleanup

```bash
# Remove everything (will ask for confirmation)
./botanix-local.sh clean
```

## Configuration Reference

### Environment Variables (.env)

#### Node Configuration
- `NUM_NODES`   - Number of federation validator nodes (default: 3)
- `MIN_SIGNERS` - Minimum signers for FROST multisig (default: 2)
- `MAX_SIGNERS` - Maximum signers for FROST multisig (default: 3)

#### Paths
- `OUTPUT_PATH` - Output directory for generated files (default: `docker-local`)
- `CONFIG_PATH` - Path for node configs (default: `docker-local/configs`)

#### Docker
- `DOCKER_SUBNET` - Docker network subnet (default: `172.22.0.0/16`)
- `PROJECT_PREFIX` - Prefix for Docker resources (default: `botanix`)

#### Blockchain
- `BLOCK_FEE_RECIPIENT_ADDRESS` - Address to receive block fees (default: `0xF27a6Ea4a1d5f7341Da7EDAaa47C5C933b738f4F`)




## System Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Docker Network: botanix-local            │
│                                                             │
│  ┌────────────┐                                             │
│  │ Bitcoin    │  Regtest node with RPC/ZMQ                  │
│  │ Core       │                                             │
│  └─────┬──────┘                                             │
│        │                                                    │
│  ┌─────┴──────────────────────────────────────────┐         │
│  │                                                │         │
│  │  Per Node (1 to N):                            │         │
│  │                                                │         │
│  │  ┌──────────────┐                              │         │
│  │  │ btc-server   │  Bitcoin signing server      │         │
│  │  └──────┬───────┘                              │         │
│  │         │                                      │         │
│  │  ┌──────┴───────┐                              │         │
│  │  │ POA (reth)   │  Proof of Authority node     │         │
│  │  └──────┬───────┘  (EVM execution)             │         │
│  │         │                                      │         │
│  │  ┌──────┴───────┐                              │         │
│  │  │ CometBFT     │  Consensus layer             │         │
│  │  └──────────────┘  (connects via ABCI)         │         │
│  │                                                │         │
│  └────────────────────────────────────────────────┘         │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## Troubleshooting

### Services won't start

```bash
# Check Docker is running
docker ps

# Check network exists
docker network ls | grep botanix

# View detailed logs
./botanix-local.sh logs
```


### Port conflicts

If you have port conflicts, edit `.env`:

```bash
# Use different output path or change the ports in generate-compose.sh
OUTPUT_PATH=docker-local-alt
```

### Bitcoin not ready

```bash
# Check bitcoin logs
./botanix-local logs bitcoin-core

# Manually re-run init
./docker-local/init-bitcoin.sh
```


### Testing Changes

```bash
# Stop services
./botanix-local.sh stop

# After code changes...

# Clean rebuild (if needed)
./botanix-local.sh clean
./botanix-local.sh start
```

### Debugging a Specific Node Issues

```bash
# View logs
./botanix-local.sh logs poa-1

# Access container
./botanix-local.sh exec poa-1

# Inside container, check configs
ls -la /reth/botanix_testnet/config/
cat /reth/botanix_testnet/config/federation.toml
```

## Advanced Usage

### Custom Node Count

Edit `.env`:
```bash
NUM_NODES=3
MIN_SIGNERS=2
MAX_SIGNERS=3
```

Then restart:
```bash
./botanix-local clean
./botanix-local start
```

### Manual Control

You can still use the underlying scripts directly:

```bash
# Just generate configs
./setup-local-env.sh --num-nodes=3

# Just generate compose file
./generate-compose.sh 3 docker-local docker-local/configs botanix

# Manual docker compose
docker compose -f docker-local/docker-compose-generated.yml up -d
```



## Getting Help

```bash
# Show help
./botanix-local.sh help

# Check service status
./botanix-local.sh services

# View all logs
./botanix-local.sh logs
```

## Notes

- All data is stored in `docker-local/` and is gitignored
- Bitcoin operates in regtest mode (isolated test network)
- Services are configured to restart automatically unless stopped
- The first `start` command may take longer as it builds Docker images
- `.env` file is gitignored - safe for local customization
