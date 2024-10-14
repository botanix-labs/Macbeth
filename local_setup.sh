#!/bin/bash
TEMP_DIR="./tmp"
# crate temp directory in project source.
mkdir -p "$TEMP_DIR"

NODE1_DIR="$TEMP_DIR/node1"
NODE2_DIR="$TEMP_DIR/node2"
BITCOIN_NODE_DIR="$TEMP_DIR/bitcoin_node"
COMET_NODE_DIR="$TEMP_DIR/comet"
WALLET_NAME="my_wallet" 

init() {
      DEFAULT_BINARY_PATH="{PROVIDE_BINARIES_PATH}"
      BINARY_PATH=${1:-$DEFAULT_BINARY_PATH}
     echo "Using binary path: $BINARY_PATH"
    if [[ ! -d "$BINARY_PATH" ]]; then
        echo "Error: Binary path '$BINARY_PATH' does not exist."
        exit 1
    fi

    # Check if the directories already exist
    if [[ -d "$NODE1_DIR" || -d "$NODE2_DIR" || -d "$BITCOIN_NODE_DIR" ]]; then
        echo "Error: One or more directories already exist."
        echo "Node1 directory: $NODE1_DIR"
        echo "Node2 directory: $NODE2_DIR"
        echo "Bitcoin Node directory: $BITCOIN_NODE_DIR"
        exit 1
    fi

    # Create the directories under $HOME
    mkdir -p "$NODE1_DIR"
    mkdir -p "$NODE2_DIR"
    mkdir -p "$BITCOIN_NODE_DIR"
    mkdir -p "$COMET_NODE_DIR"

    # Provide confirmation
    echo "Directories created:"
    echo "$NODE1_DIR"
    echo "$NODE2_DIR"
    echo "$BITCOIN_NODE_DIR"
    echo "$COMET_NODE_DIR"

    # Prompt user for RPC credentials and port, with default values
    read -p "Enter RPC username [default: test123]: " RPC_USER
    RPC_USER=${RPC_USER:-test123}

    read -p "Enter RPC password [default: test123]: " RPC_PASSWORD
    RPC_PASSWORD=${RPC_PASSWORD:-test123}

    read -p "Enter RPC port [default: 18443]: " RPC_PORT
    RPC_PORT=${RPC_PORT:-18443}

    # Create the .env file if it doesn't exist
    ENV_FILE=".env"
    if [[ -f "$ENV_FILE" ]]; then
        echo ".env file already exists."
    else
        echo "Creating .env file with Bitcoin regtest configuration and node directories..."
        cat <<EOL > $ENV_FILE
# Bitcoin Regtest Configuration
BITCOIND_NETWORK=regtest
BITCOIND_URL=http://localhost:$RPC_PORT
BITCOIND_USER=$RPC_USER
BITCOIND_PWD=$RPC_PASSWORD
BITCOIND_PORT=$RPC_PORT

# local directories
NODE_1_DIR=$NODE1_DIR
NODE_2_DIR=$NODE2_DIR
NTP_SERVER_URL=time.cloudflare.com
EOL
        echo ".env file created with Bitcoin regtest configuration and node directories."
    fi

    # Create bitcoin.conf file in the bitcoin_node folder with updated configuration
    BITCOIN_CONF="$BITCOIN_NODE_DIR/bitcoin.conf"
    echo "Creating bitcoin.conf file with updated configuration..."
    cat <<EOL > $BITCOIN_CONF
datadir=$BITCOIN_NODE_DIR
regtest=1
server=1
txindex=1
fallbackfee=0.00001

[regtest]
rpcuser=$RPC_USER
rpcpassword=$RPC_PASSWORD
rpcport=$RPC_PORT
EOL
    echo "bitcoin.conf file created at $BITCOIN_CONF with updated configuration."

    # Run the CometBFT testnet command
    echo "Running 'cometbft testnet' command..."
    "$BINARY_PATH"/cometbft testnet --o "$COMET_NODE_DIR" --v 2
    echo "CometBFT testnet initialized in $COMET_NODE_DIR."
}


start() {
DEFAULT_BINARY_PATH="{PROVIDE_BINARIES_PATH}"
BINARY_PATH=${1:-$DEFAULT_BINARY_PATH}
    echo "Starting bitcoind with configuration from $BITCOIN_NODE_DIR/bitcoin.conf..."
    "$BINARY_PATH"/bitcoind -datadir="$BITCOIN_NODE_DIR" -conf="bitcoin.conf" &

  if [[ -f ".env" ]]; then
        echo "Loading environment variables from .env file..."
        source .env
    else
        echo "Error: .env file not found. Please run the init function first."
        exit 1
    fi
sleep 5

echo "Creating wallet '$WALLET_NAME' in $BITCOIN_NODE_DIR..."

curl --user "$BITCOIND_USER:$BITCOIND_PWD" \
     --header "Content-Type: application/json" \
     --data '{"method": "createwallet", "params": ["'"$WALLET_NAME"'"], "jsonrpc": "2.0", "id": "curltest"}' \
     http://localhost:$BITCOIND_PORT/

echo ""

echo "Loading wallet '$WALLET_NAME'..."

curl --user "$BITCOIND_USER:$BITCOIND_PWD" \
     --header "Content-Type: application/json" \
     --data '{"method": "createwallet", "params": ["'"$WALLET_NAME"'"], "jsonrpc": "2.0", "id": "curltest"}' \
     http://localhost:$BITCOIND_PORT/

echo ""

echo "Generating 200 blocks..."

ADDRESS=$(curl --user "$BITCOIND_USER:$BITCOIND_PWD" \
               --header "Content-Type: application/json" \
               --data '{"method": "getnewaddress", "params": [], "jsonrpc": "2.0", "id": "curltest"}' \
               http://localhost:$BITCOIND_PORT/wallet/$WALLET_NAME | jq -r '.result')

echo "New address: $ADDRESS"

curl --user "$BITCOIND_USER:$BITCOIND_PWD" \
     --header "Content-Type: application/json" \
     --data '{"method": "generatetoaddress", "params": [200, "'"$ADDRESS"'"], "jsonrpc": "2.0", "id": "curltest"}' \
     http://localhost:$BITCOIND_PORT/

echo ""

    sleep 5
    echo "Running 'make make start-btc-server_1'..."
    make start-btc-server-1 &      
    sleep 10
    echo "Running 'make make start-btc-server_2'..."
    make start-btc-server-2 &  

    sleep 10
    echo "Running 'make start-poa-server_1' ..."
    make make start-poa-server-1  &
    sleep 10
    echo "Running 'make start-poa-server_2' ..."
    make make start-poa-server-2 &

     sleep 10
     echo "Starting CometBFT node0..."
     "$BINARY_PATH"/cometbft start --home "$COMET_NODE_DIR/node0" &
    sleep 5
    echo "Starting CometBFT node1..."
    "$BINARY_PATH"/cometbft start --home "$COMET_NODE_DIR/node1" &
    echo "CometBFT node started with home directory $HOME_DIR/".
}

stop() {
    echo "Cleaning up..."
    pkill -9 -f bitcoind
    pkill -9 -f btc-server
    pkill -9 -f cometbft
    pkill -9 -f reth
    
}

clean() {
    rm -rf "$NODE1_DIR" "$NODE2_DIR" "$BITCOIN_NODE_DIR" "$COMET_NODE_DIR" ".env" "$TEMP_DIR"
    echo "Deleted $NODE1_DIR, $NODE2_DIR, $BITCOIN_NODE_DIR, and .env file. $COMET_NODE_DIR"
}

# Main script logic to handle "init", "start", and "clean" commands
if [[ "$1" == "init" ]]; then
    init
elif [[ "$1" == "start" ]]; then
    start
elif [[ "$1" == "stop" ]]; then
    stop
elif [[ "$1" == "clean" ]]; then
    clean
else
    echo "Usage: $0 {init|start|clean}"
    exit 1
fi
