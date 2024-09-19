#!/bin/bash

set -e

echo "CometBFT already initialized, reading and applying configurations..."
exec cometbft node \
    --home=/cometbft \
    --proxy_app="${PROXY_APP:-}" \
    --moniker="${MONIKER:-}" \
    --p2p.persistent_peers="${PERSISTENT_PEERS:-}" \
    --p2p.laddr="${P2P_LADDR:-}" \
    --rpc.laddr="${RPC_LADDR:-}"
echo "CometBFT initialization done!."
