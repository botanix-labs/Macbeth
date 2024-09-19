#!/bin/bash

set -e

if [ ! -f "/cometbft/config/priv_validator_key.json" ] || [ ! -f "/cometbft/config/node_key.json" ]; then
    echo "Initializing CometBFT..."
    cometbft init -k "secp256k1" --home /cometbft
else
    echo "CometBFT already initialized, skipping init process..."
    exec cometbft node \
        --home=/cometbft \
        --proxy_app="${PROXY_APP:-}" \
        --moniker="${MONIKER:-}" \
        --p2p.persistent_peers="${PERSISTENT_PEERS:-}" \
        --p2p.laddr="${P2P_LADDR:-}" \
        --rpc.laddr="${RPC_LADDR:-}"
fi
