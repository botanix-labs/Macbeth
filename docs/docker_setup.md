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

1. Generate network configs with 2/2/2 nodes:

```bash
make init-docker-local
```

Use `NODES_NUMBER` to specify the number of nodes in the federation.
For example, to create a 3/3/3 federation, set `NODES_NUMBER=3`.

Use `botanix-up` directly to specify max/min signers count.

```bash

2. Start local-federation:

```bash
make start-docker-local
```

3. To stop the local federation, use:

```bash
make stop-docker-local
```

4. To drop all data and configs of the local federation:

```bash
make clean-docker-local
```

5. To rebuild docker images for the local federation:

```bash
make build-docker-local
```

6. To reset data in local federation:

```bash
make reset-docker-local
```

> > To build poa-node locally for feature or refactor testing use `make build-docker-local`

## Known issues

- This setup is not tested on Windows.
- You may see `permission denied` errors running `make clean-docker-local` on Ubuntu.
  Please use `sudo` to remove CometBFT data files.

## Getting Help

If you have any questions, first see if the answer to your question can be found in the [book][https://docs.botanixlabs.xyz/botanix-labs/].

If the answer is not there:

-   Join the [Telegram](https://botanixlabs.xyz/en/home) to get help, or
-   Open a [discussion](https://github.com/botanix-labs/Macbeth/issues/new) with your question, or
-   Open an issue with [the bug](https://github.com/botanix-labs/Macbeth/issues)

## Security

See [`SECURITY.md`](./SECURITY.md).
