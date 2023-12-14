# Bitcoin Signing Server

Bitcoin signer service that interacts with a database and performs transaction signing and related operations. Below is an overview of the main components and functionality provided by this code.

### Building and Running

    Ensure you have Rust and Cargo installed on your system.

    Install the required dependencies specified in your Cargo.toml file.

    First copy the template key hex
    `cp key.template.hex key.hex`

    To run the application, execute the main function using the tokio runtime. For example:

    ```bash
        cargo run -- --pkey <private_key_path> --db <db_path> --network <network_name>
    ```

    For example:
    ```bash
        cargo run -- --network testnet --pkey key.hex --db "./db"
    ```

    To just generate the client. run `cargo build`.

Take a look at the example client for example on how to execute commands against the Bitcoin signer service.