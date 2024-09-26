#!/usr/bin/env bash

set -e
set -o pipefail

# Define the list of strings
# "e2e_peer_disconnect" is temporarily disabled as we are waiting for Reth's fix here: https://github.com/paradigmxyz/reth/issues/10016
# "batch_pegins" "block_builder" "utxo_sync" "frost_e2e_stable" "frost_e2e_failed_signing_disconnect" "invalid_pegin" "invalid_pegout" "test_mempool_gossip" "rpc_node"
# Add these back in as we fix the test suite
# Add back in "many_inputs_signing" after we fix the actual test
tests_to_run=("dkg_flow" "utxo_commitment" )

exit_codes=()

# Loop over each string
for test in "${tests_to_run[@]}"; do
  # Set the environment variable
  export TEST_TO_RUN="$test"

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
