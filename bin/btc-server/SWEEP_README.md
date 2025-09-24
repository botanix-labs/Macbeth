# Sweep CLI Tool

The Sweep CLI tool is a command-line utility for creating and processing FROST threshold signature sweep transactions in the Botanix network. It is intended to be used as a backup last resort solution to transfer funds. The coordinator should not be processing new checkpoints while the sweep tool is being used (to prevent double spending of utxos).

## Overview

This process uses a 5-phase workflow that coordinates between multiple signers through JSON file exchanges.

## Installation

Build the sweep CLI tool:

```bash
cargo build --package btc-server --bin sweep
```

Or run directly:

```bash
cargo run --package btc-server --bin sweep -- <command> [args...]
```

## Workflow

The sweep process follows a 5-phase FROST threshold signature workflow:

### Phase 1: Create PSBT (Coordinator)
The coordinator creates a Partially Signed Bitcoin Transaction (PSBT) that includes all available UTXOs (up to 1000 utxos at a time).

```bash
cargo run --package btc-server --bin sweep -- coordinator-1-create-psbt \
  --db <database_path> \
  --output-address <destination_address> \
  --sat-per-vbyte <fee_rate> \
  --testnet
```

**Parameters:**
- `--db`: Path to the database containing UTXOs and key packages
- `--output-address`: Bitcoin address to receive all swept funds
- `--sat-per-vbyte`: Fee rate in satoshis per virtual byte
- `--testnet`: Flag indicating testnet addresses (omit for mainnet)

**Output:** Creates `psbt.json` and `signing_package.json` files

### Phase 2: Generate Commitments (Each Signer)
Each signer generates FROST Round 1 commitments for their portion of the signature.

```bash
cargo run --package btc-server --bin sweep -- signer-1-generate-commitments \
  --input-json signing_package.json \
  --db <signer_database_path> \
  --identifier <signer_id>
```

**Parameters:**
- `--input-json`: The signing package JSON from Phase 1
- `--db`: Path to the signer's database containing their key package
- `--identifier`: Unique numeric identifier for this signer (0, 1, 2, etc.)

**Output:** Creates `round_1_response_<frost_id>.json` and `nonces_<frost_id>.json` files

### Phase 3: Collect Commitments (Coordinator)
The coordinator collects Round 1 responses from all participating signers.

```bash
cargo run --package btc-server --bin sweep -- coordinator-2-collect-commitments \
  --round1-responses round_1_response_abc123.json,round_1_response_def456.json,round_1_response_ghi789.json \
  --min-signers 3 \
  --output-json signing_package_round2.json \
  --db <database_path>
```

**Parameters:**
- `--round1-responses`: Comma-separated list of Round 1 response JSON files
- `--min-signers`: Minimum number of signers required for the threshold
- `--output-json`: Output file for the Round 2 signing package
- `--db`: Database path for validation

**Output:** Creates `signing_package_round2.json`

### Phase 4: Generate Signatures (Selected Signers)
Selected signers (at least `min-signers` count) generate their partial signatures.

```bash
cargo run --package btc-server --bin sweep -- signer-2-generate-signatures \
  --input-json signing_package_round2.json \
  --nonces-json nonces_<frost_id>.json \
  --db <signer_database_path> \
  --identifier <signer_id>
```

**Parameters:**
- `--input-json`: The Round 2 signing package from Phase 3
- `--nonces-json`: The nonces file saved from Phase 2
- `--db`: Path to the signer's database
- `--identifier`: The signer's unique identifier

**Output:** Creates `round_2_response_<frost_id>.json`

### Phase 5: Finalize Transaction (Coordinator)
The coordinator combines all partial signatures to create the final signed transaction.

```bash
cargo run --package btc-server --bin sweep -- coordinator-3-finalize-transaction \
  --round2-responses round_2_response_abc123.json,round_2_response_def456.json,round_2_response_ghi789.json \
  --min-signers 3 \
  --output-file finalized_transaction.hex \
  --db <database_path>
```

**Parameters:**
- `--round2-responses`: Comma-separated list of Round 2 response JSON files
- `--min-signers`: Minimum number of signers required
- `--output-file`: Output file for the finalized transaction hex
- `--db`: Database path for validation

**Output:** Creates a hex-encoded signed transaction ready for broadcast

## Testing Commands

The tool includes utilities purely for testing purposes:

### Add Dummy UTXOs
```bash
cargo run --package btc-server --bin sweep -- test-add-dummy-utxos \
  --db <database_path> \
  --not-prod
```

### Generate Change Address
```bash
cargo run --package btc-server --bin sweep -- test-generate-change-address \
  --network testnet \
  --not-prod
```


## Example Complete Workflow

```bash
# Phase 1: Coordinator creates PSBT
cargo run --package btc-server --bin sweep -- coordinator-1-create-psbt \
  --db ./federation_db_0 \
  --output-address tb1pzqgyfxcjfr43rdrnr29cms873jsvzdnln5mp5vuqz3tm9n7p68yqu2mpkd \
  --sat-per-vbyte 10 \
  --testnet

# Phase 2: Each signer generates commitments
cargo run --package btc-server --bin sweep -- signer-1-generate-commitments \
  --input-json signing_package.json --db ./federation_db_0 --identifier 0

cargo run --package btc-server --bin sweep -- signer-1-generate-commitments \
  --input-json signing_package.json --db ./federation_db_1 --identifier 1

cargo run --package btc-server --bin sweep -- signer-1-generate-commitments \
  --input-json signing_package.json --db ./federation_db_2 --identifier 2

# Phase 3: Coordinator collects commitments
cargo run --package btc-server --bin sweep -- coordinator-2-collect-commitments \
  --round1-responses round_1_response_acc59f.json,round_1_response_3427a0.json,round_1_response_0cdfdb.json \
  --min-signers 3 --output-json signing_package_round2.json --db ./federation_db_0

# Phase 4: Selected signers generate signatures
cargo run --package btc-server --bin sweep -- signer-2-generate-signatures \
  --input-json signing_package_round2.json --nonces-json nonces_acc59f.json \
  --db ./federation_db_0 --identifier 0

cargo run --package btc-server --bin sweep -- signer-2-generate-signatures \
  --input-json signing_package_round2.json --nonces-json nonces_3427a0.json \
  --db ./federation_db_1 --identifier 1

cargo run --package btc-server --bin sweep -- signer-2-generate-signatures \
  --input-json signing_package_round2.json --nonces-json nonces_0cdfdb.json \
  --db ./federation_db_2 --identifier 2

# Phase 5: Coordinator finalizes transaction
cargo run --package btc-server --bin sweep -- coordinator-3-finalize-transaction \
  --round2-responses round_2_response_acc59f.json,round_2_response_3427a0.json,round_2_response_0cdfdb.json \
  --min-signers 3 --output-file finalized_sweep_transaction.hex --db ./federation_db_0
```

### File Outputs

The tool generates several types of files during execution:
- `psbt.json`: Serialized PSBT for debugging
- `signing_package.json`: Initial coordination data
- `round_1_response_<id>.json`: Signer commitments
- `nonces_<id>.json`: Private nonce data (keep secure!)
- `signing_package_round2.json`: Second round coordination data
- `round_2_response_<id>.json`: Partial signatures
- `finalized_transaction.hex`: Final signed transaction

## Integration Testing

The sweep CLI is tested end-to-end in the integration test suite. See `bin/test-suite/src/suite/consensus/frost/test_sweep_cli_e2e.rs` for a complete example of the workflow in action.