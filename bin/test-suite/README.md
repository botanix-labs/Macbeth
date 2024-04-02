# Integration Test Suite
Botanix's test-suite

### Setup
Integration tests require that bitcoind is setup in regtest mode. The tests will not setup bitcoind for you. You need to do this step first.
After you setup bitcoind in regtest update your config.toml. Use `config.template.toml` as template.

Additionally ensure you don't have any reth or btc servers running on the default ports.

### To run all tests
run from the main dir `make start-test-suite`
