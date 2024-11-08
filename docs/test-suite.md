## Testing

To run the tests:

```sh
cargo test --workspace --features all
```

We recommend using [`cargo nextest`](https://nexte.st/) to speed up testing. With nextest installed, simply substitute `cargo test` with `cargo nextest run`.

## Running integration tests

The test suite is a collection of integration tests designed to setup the full e2e stack and test various scenarios.
Take a look at the `src/suite/consensus/mod.rs` file to see the different tests that are available.

### Dependencies

Ensure you have bitcoind in your PATH. Additionally ensure you have cometbft in your PATH.
You can either install these binaries or make them from scratch.
To run the integration tests suite:

```sh
make start-test-suite
```

However, integration tests can ONLY be run using the local bitcoind instance with `regtest`. Running them on the `signet` is not feasible as block times are quite long there and the test will not finish in time. You are advised, prior to running the integration tests suite to update the `.env` file with your paths:

```bash
NODE_1_DIR=[your node 1 directory path]
NODE_2_DIR=[your node 2 directory path]
JWT_DIR=[your jwt directory path]

BITCOIND_NETWORK=[BTC NETWORK e.g. regtest]
BITCOIND_URL=[BITCOIND PROTOCOL URL WITH PORT e.g. http://localhost:18443 for regtest]
BITCOIND_USER=[USERNAME]
BITCOIND_PWD=[PASSWORD]
```
