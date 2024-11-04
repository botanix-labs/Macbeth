## Testing

To run the tests:

```sh
cargo test --workspace --features all
```

We recommend using [`cargo nextest`](https://nexte.st/) to speed up testing. With nextest installed, simply substitute `cargo test` with `cargo nextest run`.

## Running integration tests

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

## Building and pushing the Botanix images (TODO)

Pipelines do normally build and push the poA image the btc-server images to the google cloud cluster. Nevertheless, if necessary, all these images could be build and pushed manually as follows:

Build the poA image
`docker build -t botanix_testnet -f Dockerfile.testnet .`

Tag the image
`docker tag botanix_testnet:latest {region}-docker.pkg.dev/{project_id}/{repo_name}/botanix_testnet_node:latest`

For example: `docker tag botanix_testnet:latest us-central1-docker.pkg.dev/botanix-391913/botanix-testnet-node/botanix_testnet:latest`

Push the image

`docker push {region}-docker.pkg.dev/{project_id}/{repo_name}/botanix_testnet:latest`
For example: `docker push us-central1-docker.pkg.dev/botanix-391913/botanix-testnet-node/botanix_testnet:latest`
