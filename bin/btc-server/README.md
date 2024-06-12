# Bitcoin Signing Server

Bitcoin signer service that interacts with a database and performs transaction signing and related operations. Below is an overview of the main components and functionality provided by this code.

### Building and Running

    Ensure you have Rust and Cargo installed on your system.

    Run `cp config.toml template.config.toml` and ensure the config variables are correct

    Run `make start-btc-server-1` from the project root directory


    To just generate the client. run `cargo build`.

    Take a look at the integration test suite in `bin/test-suite` for examples of using the client.
