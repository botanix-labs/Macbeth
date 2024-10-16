#!/bin/bash


HOME_DIR="$HOME/develop"  

# Define the directory paths globally
NODE1_DIR="$HOME_DIR/node1"
NODE2_DIR="$HOME_DIR/node2"
BITCOIN_NODE_DIR="$HOME_DIR/bitcoin_node"
COMET_NODE_DIR="$HOME_DIR/comet"

# Function to initialize everything.
init() {
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

    # Create the .env file if it doesn't exist
    ENV_FILE=".env"
    if [[ -f "$ENV_FILE" ]]; then
        echo ".env file already exists."
    else
        echo "Creating .env file with Bitcoin regtest configuration and node directories..."
        cat <<EOL > $ENV_FILE
# Bitcoin Regtest Configuration
BITCOIND_NETWORK=regtest
BITCOIND_URL=http://localhost:18443
BITCOIND_USER=test123
BITCOIND_PWD=test123

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
rpcuser=test123
rpcpassword=test123
server=1
txindex=1
fallbackfee=0.00001
EOL
echo "bitcoin.conf file created at $BITCOIN_CONF with updated configuration."

    # Run the CometBFT testnet command
    echo "Running 'cometbft testnet' command..."
    ./binaries/cometbft testnet --o "$COMET_NODE_DIR" --v 2
    echo "CometBFT testnet initialized in $COMET_NODE_DIR."
}

# Function to start the environment and run the CometBFT testnet and Bitcoin commands
start() {
    # Call the 'init' function to ensure everything is set up before starting
    echo "Starting the bitcoin node..."
    sleep 5
    # Run Bitcoin-related commands
    echo "Starting bitcoind with configuration from $BITCOIN_NODE_DIR/bitcoin.conf..."
   ./binaries/bitcoind -datadir="$BITCOIN_NODE_DIR" -conf="$BITCOIN_NODE_DIR/bitcoin.conf" &

    # Wait for bitcoind to start
    sleep 5
    echo "Creating wallet 'mywallet' in $BITCOIN_NODE_DIR..."
    ./binaries/bitcoin-wallet -chain=regtest -wallet=mywallet -datadir="$BITCOIN_NODE_DIR" create

    echo "Loading wallet 'mywallet'..."
    ./binaries/bitcoin-cli -rpcport=18443 -rpcuser=test123 -rpcpassword=test123 loadwallet "mywallet"

    echo "Generating 200 blocks..."
    ./binaries/bitcoin-cli -rpcport=18443 -rpcuser=test123 -rpcpassword=test123 -generate 200
    
    sleep 5
    echo "Running 'make make start-btc-server_1'..."
    make start-btc-server-1 &      
    sleep 60
    echo "Running 'make make start-btc-server_2'..."
    make start-btc-server-2 &  

    sleep 10
    echo "Running 'make start-poa-server_1' ..."
    make make start-poa-server-1  &
    sleep 60
    echo "Running 'make start-poa-server_2' ..."
    make make start-poa-server-2 &

     #Run the CometBFT start command
     sleep 10
     echo "Starting CometBFT node0..."
    ./binaries/cometbft start --home "$COMET_NODE_DIR/node0" &
    sleep 5
    echo "Starting CometBFT node1..."
    ./binaries/cometbft start --home "$COMET_NODE_DIR/node1" &
    echo "CometBFT node started with home directory $HOME_DIR/".
}

# Function to clean up (delete directories and .env file)
stop() {
    echo "Cleaning up..."
    ./binaries/bitcoin-cli -regtest -rpcport=18443 -rpcuser=test123 -rpcpassword=test123 stop
     
    pkill -9 -f btc-server
    pkill -9 -f cometbft
    
    # Remove node directories and .env file
    rm -rf "$NODE1_DIR" "$NODE2_DIR" "$BITCOIN_NODE_DIR" "$COMET_NODE_DIR" ".env"

    echo "Deleted $NODE1_DIR, $NODE2_DIR, $BITCOIN_NODE_DIR, and .env file. $COMET_NODE_DIR"
}

# Main script logic to handle "init", "start", and "clean" commands
if [[ "$1" == "init" ]]; then
    init
elif [[ "$1" == "start" ]]; then
    start
elif [[ "$1" == "stop" ]]; then
    stop
else
    echo "Usage: $0 {init|start|clean}"
    exit 1
fi
