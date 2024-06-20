#!/usr/bin/env bash

set -e
set -o pipefail

# Define the list of strings
tests_to_run=("dkg_flow" "many_inputs_signing" "utxo_commitment" "block_builder" "frost_e2e_stable" "frost_e2e_failed_signing_disconnect" "invalid_pegin" "test_mempool_gossip")

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
