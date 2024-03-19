# Botanix Protocol 

[![CI status](https://github.com/paradigmxyz/reth/workflows/ci/badge.svg)]
[![cargo-deny status](https://github.com/paradigmxyz/reth/workflows/deny/badge.svg)]

**A blazing fast and secure L2 for bitcoin using the EVM as a superstructure**

![](./images/botanix.jpg)

## Requirements

To run the stack locally, please go through the following steps to ensure you have all necessary prerequisites:

1. Install `rust` (best way to install it is through the rustup toolchain: [rust](https://rustup.rs/) - depending on your OS). Default to nightly version. Minimum required version is `1.75`.
1. Install `docker` on your OS - simply follow the instructions here: [docker](https://docs.docker.com/engine/install/)
1. Install `foundry/forge` on your system - simply follow the instructions here: [foundry](https://book.getfoundry.sh/getting-started/installation)
1. Install and set up `git` to use ssh.
1. Install protobuf native dependencies: `apt update && apt upgrade -y && apt install -y protobuf-compiler libprotobuf-dev` (for ubuntu only)
1. Install libclang dependeny: `sudo apt-get install libclang-dev` (for ubuntu only)
1. Install gcp-cli: [google-cloud-cli](https://cloud.google.com/sdk/docs/install)
1. Install [k9s](https://k9scli.io/topics/install/)

## Connecting to bitcoind on the cluster

1. Install google cloud cli following the link here [gpc](https://cloud.google.com/sdk/docs/install) depending on your platform.
1. Provided you have been given access by an administrator, connect to the google cloud cluster where the bitcoind server is running using:

```shell
gcloud container clusters get-credentials botanixlabs-cluster-dev --region us-central1 --project botanix-391913
```
1. Install [k9s](https://k9scli.io/topics/install/) depending on your platform.
1. Start `k9s` using the following command:
```shell
KUBE_EDITOR=nano k9s
```
and automatically select the cluster context. Then find the pod where bitcoind is running, press `Shift+f` for port-forwarding and select the local port
onto which the pod traffic is to be forwarded. Usually that is `38332`.

## Runing a local federation

Once you have connected to bitcoind, you can run a local federation of two and more nodes easily.
These instructions set up federation nodes running poa consensus on your local set up.
Please note that the federation on feature/poA-consensus consists of at least two federation members


1. Configure `.env` file adjusting the values of the bitcoind server you want to connect to updating the values of `BITCOIND_URL`, `BITCOIND_USER`, `BITCOIND_PWD` with regards to the locally port-forwarded traffic. Ask adminstrator for username and password.

```bash
BITCOIND_URL=http://localhost:38332
BITCOIND_USER=[USER]
BITCOIND_PWD=[PWD]
```

2. Set up directories for a two different reth nodes, `[PATH_TO_NODE1]`, `[PATH_TO_NODE2]` and add a federation secret key to each of them. You can use the code snippets provided below directly.

For federation member 1:
```bash
cd [PATH_TO_NODE1] && echo "0a35afe1386497890e1dce7286a5b378b978ede20db900e6ce5b4eb1a0449ad6" > [PATH_TO_NODE1]/discovery-secret
```

For federation member 2:
```bash
cd [PATH_TO_NODE2] && echo "0cc8f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe057f094135f2c9b019" > [PATH_TO_NODE2]/discovery-secret
```

3. Start the two bitcoin servers in two separate prompts: 

```bash
make start-btc-server-1
make start-btc-server-2
```

When starting the btc servers please adjust the argument `--jwt` in the arguments list to point to the location of your jwt.hex token. The latter is to be usually found under e.g. `NODE_1_DIR/jwt.hex` and respectively for node2 `NODE_2_DIR/jwt.hex`. The btc server needs to be authenticated against the node jwt token in order to work properly.

4. Start the 2 botanix nodes as follows:

```bash
NODE_1_DIR=[PATH_TO_NODE1] make start-poa-server-1
NODE_2_DIR=[PATH_TO_NODE2] make start-poa-server-2
```

where `[PATH_TO_NODE1]` and `[PATH_TO_NODE2]` are the absolute paths to the locations where the node configurations are stored. Wait for the nodes to start and connect to the bitcoind server. Usually takes around ~10secs.

5. Connect the two federation nodes via the admin rpc endpoint:

```bash
`cast rpc admin_addPeer "enode://bdc272b244f717604fffe659d2d98205d1e6764fdf453d1631f42c2db4d8d710606084da81495d55673bfc038bdf41e3f4c17d09c875a0bcc1ea809219e34826@127.0.0.1:30304"`
```

> **Note**
>
> you need cast installed via [forge](https://book.getfoundry.sh/getting-started/installation) in order to use `cast`.

## Testing

To run the tests:

```sh
cargo test --workspace --features all
```

We recommend using [`cargo nextest`](https://nexte.st/) to speed up testing. With nextest installed, simply substitute `cargo test` with `cargo nextest run`.

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

## Getting Help

If you have any questions, first see if the answer to your question can be found in the [book][https://docs.botanixlabs.xyz/botanix-labs/].

If the answer is not there:

- Join the [Telegram](https://botanixlabs.xyz/en/home) to get help, or
- Open a [discussion](https://github.com/botanix-labs/Macbeth/issues/new) with your question, or
- Open an issue with [the bug](https://github.com/botanix-labs/Macbeth/issues)

## Security

See [`SECURITY.md`](./SECURITY.md).

## Acknowledgements

The Botanix Project team