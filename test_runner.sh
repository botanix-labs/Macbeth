#!/bin/bash

# Define the list of strings
tests_to_run=("dkg_flow" "many_inputs_signing" "utxo_commitment" "block_builder" "frost_e2e_stable" "frost_e2e_failed_signing_disconnect" "invalid_pegin" "invalid_pegout" "test_mempool_gossip")

# Loop over each string
for test in "${tests_to_run[@]}"; do
  # Set the environment variable
  export TEST_TO_RUN="$test"

  # Call make
  make start-test-suite
done
