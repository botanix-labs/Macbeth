# Botanix Protocol 

[![CI status](https://github.com/paradigmxyz/reth/workflows/ci/badge.svg)]
[![cargo-deny status](https://github.com/paradigmxyz/reth/workflows/deny/badge.svg)]

**A blazing fast and secure L2 for bitcoin using the EVM as a superstructure**


## Requirements

To run the stack locally, please go through the following steps to ensure you have all necessary prerequisites:

1. Install `rust` (best way to install it is through the rustup toolchain: [rust](https://rustup.rs/) - depending on your OS). Default to nightly version. Minimum required version is `1.75`.
1. Install `docker` on your OS - simply follow the instructions here: [docker](https://docs.docker.com/engine/install/)
1. Install `foundry/forge` on your system - simply follow the instructions here: [foundry](https://book.getfoundry.sh/getting-started/installation)
1. Install and set up `git` to use ssh.
1. Install protobuf native dependencies: `apt update && apt upgrade -y && apt install -y protobuf-compiler libprotobuf-dev` (for ubuntu only)
1. Install libclang dependency: `sudo apt-get install libclang-dev` (for ubuntu only)
1. Install gcp-cli: [google-cloud-cli](https://cloud.google.com/sdk/docs/install)
1. Install [k9s](https://k9scli.io/topics/install/)

## Codespell

On ci we run `codespell` on the codebase to ensure there are no typos.
If you want to run it locally you can install it with `pip install codespell` and then run it in the root of the repo with `codespell -w .` to automatically fix the typos.

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


## Run a local federation (Docker-Compose)

1. Change directory to `docker-local` and configure `.bitcoin.env` file adjusting the values of the bitcoind server in the docker-compose.

2. Start the bitcoind server using  `make start-docker-bitcoin`.

3. Copy `federation.template.toml` to the `docker-local/poa-1 && docker-local/poa-2` directory using:

```bash
cp federation.template.toml docker-local/poa-1/chain.toml
cp federation.template.toml docker-local/poa-2/chain.toml
```

4. Update the `chain.toml` federation members ip addresses to host machine ip to avoid network connectivity issues.

5. Start local-federation with 2/2/2 nodes using:

```bash
make start-docker-local
```

6. Update the `genesis.json` for cometbft nodes with the appropriate validator keys

7. Update the `PERSISTENT_PEERS` value for cometbft nodes with the appropriate id. To get the peer id run:

```bash
docker exec -it consensus-node-1 cometbft show-node-id --home /cometbft
docker exec -it consensus-node-2 cometbft show-node-id --home /cometbft
```

***Notes***
>> To build poa-node locally for feature or refactor testing use `make build-docker-local`

>> you need cast installed via [forge](https://book.getfoundry.sh/getting-started/installation) in order to use `cast`.




## Getting Help

If you have any questions, first see if the answer to your question can be found in the [book][https://docs.botanixlabs.xyz/botanix-labs/].

If the answer is not there:

- Join the [Telegram](https://botanixlabs.xyz/en/home) to get help, or
- Open a [discussion](https://github.com/botanix-labs/Macbeth/issues/new) with your question, or
- Open an issue with [the bug](https://github.com/botanix-labs/Macbeth/issues)

## Security

See [`SECURITY.md`](./SECURITY.md).
