#!/bin/bash
set -e

NUM_NODES=${1:-3}
OUTPUT_PATH=${2:-"docker-local"}
CONFIG_PATH=${3:-"docker-local/configs"}
PROJECT_PREFIX=${4:-"botanix"}
COMPOSE_FILE="$OUTPUT_PATH/docker-compose-generated.yml"


echo "Extracting CometBFT node IDs..."
declare -a NODE_IDS
for j in $(seq 1 $NUM_NODES); do
    NODE_CONFIG_PATH="$CONFIG_PATH/node-$j/cometbft"
    if [ -d "$NODE_CONFIG_PATH" ]; then
        NODE_ID=$(cometbft show-node-id --home="$NODE_CONFIG_PATH" 2>/dev/null || echo "")
        NODE_IDS[$j]=$NODE_ID
        echo "  Node $j ID: $NODE_ID"
    fi
done


PRIVATE_PEER_IDS=""
for j in $(seq 1 $NUM_NODES); do
    if [ -n "${NODE_IDS[$j]}" ]; then
        if [ -z "$PRIVATE_PEER_IDS" ]; then
            PRIVATE_PEER_IDS="${NODE_IDS[$j]}"
        else
            PRIVATE_PEER_IDS="$PRIVATE_PEER_IDS,${NODE_IDS[$j]}"
        fi
    fi
done

echo "Private peer IDs: $PRIVATE_PEER_IDS"

# persistent peers list with node IDs and Docker hostnames
PERSISTENT_PEERS=""
for j in $(seq 1 $NUM_NODES); do
    if [ -n "${NODE_IDS[$j]}" ]; then
        PEER="${NODE_IDS[$j]}@cometbft-$j:26656"
        if [ -z "$PERSISTENT_PEERS" ]; then
            PERSISTENT_PEERS="$PEER"
        else
            PERSISTENT_PEERS="$PERSISTENT_PEERS,$PEER"
        fi
    fi
done

echo "Persistent peers: $PERSISTENT_PEERS"

# Calculate relative path from compose file to configs
# If CONFIG_PATH is already relative and starts with OUTPUT_PATH, extract the relative part
if [[ "$CONFIG_PATH" == "$OUTPUT_PATH"* ]]; then
    RELATIVE_CONFIG_PATH=".${CONFIG_PATH#$OUTPUT_PATH}"
else
    # Convert to absolute paths if needed
    ABS_OUTPUT_PATH=$(cd "$OUTPUT_PATH" 2>/dev/null && pwd || echo "$OUTPUT_PATH")
    ABS_CONFIG_PATH=$(cd "$CONFIG_PATH" 2>/dev/null && pwd || echo "$CONFIG_PATH")
    
    RELATIVE_CONFIG_PATH="$ABS_CONFIG_PATH"
fi

cat > $COMPOSE_FILE << 'EOF'
version: '3.8'

services:
  bitcoin-core:
    image: ruimarinho/bitcoin-core:24
    container_name: bitcoin-core
    command: -regtest -server -rpcport=18443 -rpcuser=foo -rpcpassword=bar -rpcallowip=0.0.0.0/0 -rpcbind=0.0.0.0 -zmqpubhashblock=tcp://0.0.0.0:28332 -fallbackfee=0.00001 -txindex
    ports:
      - "18443:18443"
      - "38332:8332"
      - "28332:28332"
    networks:
      botanix-local:
        ipv4_address: 172.22.0.2

EOF

# Generate services for each node
for i in $(seq 1 $NUM_NODES); do
    NODE_INDEX=$((i - 1))
    NODE_NAME="node-$i"

    # port offsets
    POA_RPC_PORT=$((8545 + NODE_INDEX * 100))
    POA_WS_PORT=$((8546 + NODE_INDEX * 100))
    POA_METRICS_PORT=$((9001 + NODE_INDEX * 100))
    BTC_SERVER_PORT=$((8080 + NODE_INDEX * 100))
    BTC_SERVER_METRICS_PORT=$((7000 + NODE_INDEX * 100))
    COMET_RPC_PORT=$((26657 + NODE_INDEX * 100))
    COMET_METRICS_PORT=$((26660 + NODE_INDEX * 100))
    COMET_P2P_PORT=$((26656 + NODE_INDEX * 100))
    ABCI_PORT=26658  # Fixed ABCI port for all nodes
    FROST_MIN_SIGNERS=${FROST_MIN_SIGNERS:-2}
    FROST_MAX_SIGNERS=${FROST_MAX_SIGNERS:-3}
    BLOCK_FEE_RECIPIENT_ADDRESS=${BLOCK_FEE_RECIPIENT_ADDRESS:-0xF27a6Ea4a1d5f7341Da7EDAaa47C5C933b738f4F}

    POA_IP="172.22.$i.1"
    BTC_SERVER_IP="172.22.$i.2"
    COMETBFT_IP="172.22.$i.3"

    # Build persistent peers list for this node (excluding itself)
    NODE_PERSISTENT_PEERS=""
    for j in $(seq 1 $NUM_NODES); do
        if [ $j -ne $i ] && [ -n "${NODE_IDS[$j]}" ]; then
            PEER="${NODE_IDS[$j]}@cometbft-$j:26656"
            if [ -z "$NODE_PERSISTENT_PEERS" ]; then
                NODE_PERSISTENT_PEERS="$PEER"
            else
                NODE_PERSISTENT_PEERS="$NODE_PERSISTENT_PEERS,$PEER"
            fi
        fi
    done

    # btc-server service
    cat >> $COMPOSE_FILE << EOF
  btc-server-$i:
    build:
      context: ..
      dockerfile: Dockerfile
      args:
        PACKAGE: btc-server
        BIN: btc-server
        PROFILE: release
    container_name: btc-server-$i
    env_file:
      - $RELATIVE_CONFIG_PATH/$NODE_NAME/.env
    environment:
      - RUST_LOG=info
      - RUST_BACKTRACE=1
    command:
      - --btc-network=regtest
      - --identifier=$NODE_INDEX
      - --address=0.0.0.0:8080
      - --db=/bitcoin-server/data/db
      - --min-signers=$FROST_MIN_SIGNERS
      - --max-signers=$FROST_MAX_SIGNERS
      - --toml=/bitcoin-server/config/config.toml
      - --fee-rate-diff-percentage=30
      - --bitcoind-url=http://bitcoin-core:18443
      - --bitcoind-user=foo
      - --bitcoind-pass=bar
      - --btc-signing-server-jwt-secret=/bitcoin-server/config/bjwt.hex
      - --fall-back-fee-rate-sat-per-vbyte=5
      - --metrics-port=$BTC_SERVER_METRICS_PORT
      - --federation-config-path=/bitcoin-server/config/federation.toml
      - --p2p-secret-key=/bitcoin-server/config/discovery-secret
      - --coordinator=0
    volumes:
      - $RELATIVE_CONFIG_PATH/$NODE_NAME/btc_server:/bitcoin-server/config
      - $RELATIVE_CONFIG_PATH/$NODE_NAME/btc_server/data:/bitcoin-server/data
    ports:
      - "$BTC_SERVER_PORT:8080"
    networks:
      botanix-local:
        ipv4_address: $BTC_SERVER_IP
    depends_on:
      - bitcoin-core
    restart: unless-stopped

  poa-$i:
    build:
      context: ..
      dockerfile: Dockerfile
      args:
        PACKAGE: reth
        BIN: reth
        PROFILE: release
    container_name: poa-$i
    env_file:
      - $RELATIVE_CONFIG_PATH/$NODE_NAME/.env
    environment:
      - RUST_LOG=info
      - RUST_BACKTRACE=1
    command:
      - poa
      - --federation-config-path=/reth/botanix_testnet/config/federation.toml
      - --datadir=/reth/botanix_testnet/data
      - --http
      - --http.addr=0.0.0.0
      - --http.port=8545
      - --http.api=debug,eth,net,trace,txpool,web3,rpc
      - --http.corsdomain=*
      - --ws
      - --ws.port=8546
      - -vvv
      - --btc-signing-server-jwt-secret=/reth/botanix_testnet/config/bjwt.hex
      - --btc-server=$BTC_SERVER_IP:8080
      - --bitcoind.url=http://bitcoin-core:18443
      - --bitcoind.username=foo
      - --bitcoind.password=bar
      - --frost.min_signers=$FROST_MIN_SIGNERS
      - --frost.max_signers=$FROST_MAX_SIGNERS
      - --p2p-secret-key=/reth/botanix_testnet/config/discovery-secret
      - --port=30303
      - --btc-network=regtest
      - --metrics=0.0.0.0:9001
      - --federation-mode
      - --abci-port=26658
      - --ipcdisable
      - --abci-host=0.0.0.0
      - --cometbft-rpc-port=26657
      - --cometbft-rpc-host=$COMETBFT_IP
      - --block-fee-recipient-address=$BLOCK_FEE_RECIPIENT_ADDRESS
      - --is-testnet
    volumes:
      - $RELATIVE_CONFIG_PATH/$NODE_NAME/poa:/reth/botanix_testnet/config
      - $RELATIVE_CONFIG_PATH/$NODE_NAME/poa/data:/reth/botanix_testnet/data
    ports:
      - "$POA_RPC_PORT:8545"
      - "$POA_WS_PORT:8546"
      - "$POA_METRICS_PORT:9001"
    networks:
      botanix-local:
        ipv4_address: $POA_IP
    depends_on:
      - btc-server-$i
      - bitcoin-core
    restart: unless-stopped

  cometbft-$i:
    image: cometbft/cometbft:v1.0.0
    container_name: cometbft-$i
    env_file:
      - $RELATIVE_CONFIG_PATH/$NODE_NAME/.env
    command: >
      node
      --home=/cometbft
      --proxy_app="$POA_IP:$ABCI_PORT"
      --moniker="$NODE_NAME"
      --p2p.persistent_peers='$NODE_PERSISTENT_PEERS'
      --p2p.laddr=tcp://0.0.0.0:26656
      --rpc.laddr=tcp://0.0.0.0:26657
      --log_level="p2p:debug,state:debug,consensus:debug"
    volumes:
      - $RELATIVE_CONFIG_PATH/$NODE_NAME/cometbft:/cometbft:rw
    ports:
      - "$COMET_RPC_PORT:26657"
      - "$COMET_METRICS_PORT:26660"
    networks:
      botanix-local:
        ipv4_address: $COMETBFT_IP
        aliases:
          - cometbft-$i
    depends_on:
      - poa-$i
    restart: unless-stopped

EOF
done

cat >> $COMPOSE_FILE << EOF
networks:
  botanix-local:
    external: true
    name: ${PROJECT_PREFIX}-local
EOF

echo "Generated $COMPOSE_FILE with $NUM_NODES nodes"
