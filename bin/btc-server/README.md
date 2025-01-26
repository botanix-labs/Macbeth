# Bitcoin Signing Server

Bitcoin signer service that interacts with a database and performs transaction signing and related operations. Below is an overview of the main components and functionality provided by this code.

## Building and Running

Ensure you have Rust and Cargo installed on your system.

Run `make start-btc-server-1` from the project root directory

To just generate the client. run `cargo build --features conflicting_input`.

Take a look at the integration test suite in `bin/test-suite` for examples of using the client.

## Troubleshooting

if you have having issues with the grpc reflection server, for example:
`Grpc server: Join Error grpc reflection server error: error decoding FileDescriptorSet from buffer`

This is likely because the file descriptor set is not being generated correctly. To re-generate delete src/rpc/btc_server.bin and make some arbitrary change to the source code and run `cd bin/btc-server && cargo build`.
