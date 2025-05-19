#!/usr/bin/env bash

set -e
set -o pipefail

# Define the list of strings
# "e2e_peer_disconnect" is temporarily disabled as we are waiting for Reth's fix here: https://github.com/paradigmxyz/reth/issues/10016
# "rpc_node" "batch_pegins" "frost_e2e_failed_signing_disconnect" "frost_e2e_failed_signing"
# Add these back in as we fix the test suite
tests_to_run=("dkg_flow" "utxo_commitment" "signing_flow" "test_mempool_gossip" "utxo_sync" "state_sync" "wallet_sync" "frost_e2e_stable" "test_pegin_v1" "invalid_pegin" "invalid_pegout" "block_builder" "test_conflicting_input" "test_round1_then_new_signing_session" "test_track_mempool" "rpc_node")

exit_codes=()

# Loop over each string
for test in "${tests_to_run[@]}"; do
    # Set the environment variable
    export TEST_TO_RUN="$test"

    # kill all btc-servers that may be running from previous test runs
    killall btc-server || true

    # kill all bitcoind instances that may be running from previous test runs
    killall bitcoind || true

    # Call make
    make start-test-suite
    exit_codes+=("$?")
done

# Print the exit codes at the end
echo "Exit codes for each test:"
for i in "${!tests_to_run[@]}"; do
    echo "${tests_to_run[$i]}: ${exit_codes[$i]}"
done

# Exit with 1 if any test failed
if [[ " ${exit_codes[@]} " =~ " 1 " ]]; then
    exit 1
fi

exit 0
