#!/bin/bash
set -e

# Default values
NUM_NODES=3
OUTPUT_PATH="docker-local"
CONFIG_PATH="docker-local/configs"
MIN_SIGNERS="2"
MAX_SIGNERS="3"
DOCKER_SUBNET="172.22.0.0/16"
PROJECT_PREFIX="botanix"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --num-nodes=*)
            NUM_NODES="${1#*=}"
            shift
            ;;
        --output-path=*)
            OUTPUT_PATH="${1#*=}"
            shift
            ;;
        --min-signers=*)
            MIN_SIGNERS="${1#*=}"
            shift
            ;;
        --max-signers=*)
            MAX_SIGNERS="${1#*=}"
            shift
            ;;
        --subnet=*)
            DOCKER_SUBNET="${1#*=}"
            shift
            ;;
        --prefix=*)
            PROJECT_PREFIX="${1#*=}"
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--num-nodes=N] [--output-path=PATH] [--min-signers=N] [--max-signers=N] [--subnet=CIDR] [--prefix=NAME]"
            exit 1
            ;;
    esac
done

# defaults for min/max signers if not provided
if [ -z "$MIN_SIGNERS" ]; then
    MIN_SIGNERS=$NUM_NODES
fi
if [ -z "$MAX_SIGNERS" ]; then
    MAX_SIGNERS=$NUM_NODES
fi

echo "Setting up local docker environment..."
echo "  Nodes: $NUM_NODES"
echo "  Output: $OUTPUT_PATH"
echo "  Config: $CONFIG_PATH"
echo "  Min Signers: $MIN_SIGNERS"
echo "  Max Signers: $MAX_SIGNERS"
echo "  Subnet: $DOCKER_SUBNET"
echo ""

# Create output directory if it doesn't exist
mkdir -p "$OUTPUT_PATH"

# Clean up existing configs directory
if [ -d "$CONFIG_PATH" ]; then
    echo "Cleaning existing config directory..."
    rm -rf "$CONFIG_PATH"
fi

# Run botanix-up to generate configs
echo "Generating node configurations..."
cargo run -p botanix-up -- \
    --num-nodes=$NUM_NODES \
    --output-path=$CONFIG_PATH \
    --multisig-min-signers=$MIN_SIGNERS \
    --multisig-max-signers=$MAX_SIGNERS \
    --docker-subnet=$DOCKER_SUBNET \
    --project-name-prefix=$PROJECT_PREFIX

echo ""
echo "Configuration generated successfully in $CONFIG_PATH"

# correct genesis.json mismatch (botanix-up generates different genesis files per node)
echo "Fixing genesis.json files..."
MASTER_GENESIS="$CONFIG_PATH/node-1/cometbft/config/genesis.json"
if [ -f "$MASTER_GENESIS" ]; then
    echo "  Using node-1 genesis as master"
    for NODE_DIR in "$CONFIG_PATH"/node-*/cometbft/config; do
        if [ "$NODE_DIR" != "$CONFIG_PATH/node-1/cometbft/config" ]; then
            cp "$MASTER_GENESIS" "$NODE_DIR/genesis.json"
            echo "Copied genesis to $(basename $(dirname $(dirname $NODE_DIR)))"
        fi
    done
else
    echo "  Warning: Master genesis file not found"
fi

# correct persistent_peers in config.toml (botanix-up uses wrong hostnames)
echo "Fixing persistent_peers in config.toml files..."
for NODE_DIR in "$CONFIG_PATH"/node-*/cometbft/config; do
    if [ -f "$NODE_DIR/config.toml" ]; then
        sed -i.bak 's/^persistent_peers = .*/persistent_peers = ""/' "$NODE_DIR/config.toml"
        echo " Cleared persistent_peers in $(basename $(dirname $(dirname $NODE_DIR)))"
    fi
done


NETWORK_NAME="${PROJECT_PREFIX}-local"
if ! docker network inspect $NETWORK_NAME > /dev/null 2>&1; then
    echo "Creating docker network: $NETWORK_NAME"
    docker network create --subnet=$DOCKER_SUBNET $NETWORK_NAME
else
    echo "Docker network $NETWORK_NAME already exists"
fi


echo "Generating docker-compose.yml..."
./generate-compose.sh $NUM_NODES $OUTPUT_PATH $CONFIG_PATH $PROJECT_PREFIX

# Generate bitcoin initialization script
echo "Generating bitcoin initialization script..."
cat > "$OUTPUT_PATH/init-bitcoin.sh" << 'INIT_SCRIPT'
#!/bin/bash
set -e

WALLET_NAME="testwallet"
BLOCKS_TO_GENERATE=1000

echo "Waiting for bitcoin-core to be ready..."
until docker exec bitcoin-core bitcoin-cli -regtest -rpcuser=foo -rpcpassword=bar getblockchaininfo &> /dev/null; do
    echo "Bitcoin node not ready yet, waiting..."
    sleep 2
done

echo "Bitcoin node is ready!"

# Check if wallet already exists
if docker exec bitcoin-core bitcoin-cli -regtest -rpcuser=foo -rpcpassword=bar listwallets | grep -q "$WALLET_NAME"; then
    echo "Wallet '$WALLET_NAME' already exists"
else
    echo "Creating wallet '$WALLET_NAME'..."
    docker exec bitcoin-core bitcoin-cli -regtest -rpcuser=foo -rpcpassword=bar createwallet "$WALLET_NAME"
fi

# Load wallet if not loaded
echo "Loading wallet '$WALLET_NAME'..."
docker exec bitcoin-core bitcoin-cli -regtest -rpcuser=foo -rpcpassword=bar loadwallet "$WALLET_NAME" 2>/dev/null || echo "Wallet already loaded"

echo "Generating address..."
ADDRESS=$(docker exec bitcoin-core bitcoin-cli -regtest -rpcuser=foo -rpcpassword=bar -rpcwallet="$WALLET_NAME" getnewaddress)
echo "Generated address: $ADDRESS"

echo "Generating $BLOCKS_TO_GENERATE blocks to $ADDRESS..."
docker exec bitcoin-core bitcoin-cli -regtest -rpcuser=foo -rpcpassword=bar -rpcwallet="$WALLET_NAME" generatetoaddress $BLOCKS_TO_GENERATE "$ADDRESS"

BLOCK_COUNT=$(docker exec bitcoin-core bitcoin-cli -regtest -rpcuser=foo -rpcpassword=bar getblockcount)
echo "Current block height: $BLOCK_COUNT"

BALANCE=$(docker exec bitcoin-core bitcoin-cli -regtest -rpcuser=foo -rpcpassword=bar -rpcwallet="$WALLET_NAME" getbalance)
echo "Wallet balance: $BALANCE BTC"

echo ""
echo "Bitcoin initialization complete!"
echo "Wallet: $WALLET_NAME"
echo "Address: $ADDRESS"
echo "Blocks: $BLOCK_COUNT"
echo "Balance: $BALANCE BTC"
INIT_SCRIPT

chmod +x "$OUTPUT_PATH/init-bitcoin.sh"

echo ""
echo "Setup complete!"
echo ""
echo "To start the environment:"
echo "  1. Start services: docker compose -f $OUTPUT_PATH/docker-compose-generated.yml up -d"
echo "  2. Initialize bitcoin: $OUTPUT_PATH/init-bitcoin.sh"
echo ""
echo "Or run both in one command:"
echo "  docker compose -f $OUTPUT_PATH/docker-compose-generated.yml up -d && sleep 5 && $OUTPUT_PATH/init-bitcoin.sh"
