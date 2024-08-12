# Docker

Our installation docs support running rpc-nodes via docker compose.
In the future we will provide federation member support, also via docker compose.

> **Note**
>
> Reth requires Docker Engine version 20.10.10 or higher due to [missing support](https://docs.docker.com/engine/release-notes/20.10/#201010) for the `clone3` syscall in previous versions.

## GitHub

Botanix docker images are released on docker hub.

You can obtain the latest image with:

```bash
docker pull us-central1-docker.pkg.dev/botanix-391913/botanix-testnet-node-v1/botanix-poa-node
```

Or a specific version (e.g. v0.0.1) with:

```bash
docker pull us-central1-docker.pkg.dev/botanix-391913/botanix-testnet-node-v1/botanix-poa-node:v.0.0.1
```

### Using Docker Compose

This setup provides a environment for running a Bitcoin Core node, a Botanix Rpc node, and monitoring tools using Grafana Alloy. The services are configured to work together, with appropriate dependencies and ports exposed for interaction.

```docker
version: '3.7'
services:
  bitcoin-core:
    env_file:
      - .bitcoin.env
    image: ruimarinho/bitcoin-core:latest
    hostname: bitcoin-core
    container_name: bitcoin-core
    command:
      - -printtoconsole
      - -signet=1
      - -txindex=1
      - -server=1
      - -rpcport=38332
      - -rpcuser=${BITCOIND_USER}
      - -rpcpassword=${BITCOIND_PASS}
      - -rpcbind=0.0.0.0
      - -rpcallowip=0.0.0.0/0
      - -blockfilterindex=1
    volumes:
      - ./btc:/home/bitcoin/.bitcoin
      - ./bitcoin.conf:/home/bitcoin/.bitcoin/bitcoin.conf
    ports:
      - 38332:38332
      - 38333:38333
      - 38334:38334

  poa-node-rpc:
    env_file:
      - .bitcoin.env
    container_name: botanix-poa-node-rpc
    image:  us-central1-docker.pkg.dev/botanix-391913/botanix-testnet-node-v1/botanix-poa-node
    command:
      - poa
      - --federation-config-path=/reth/botanix_testnet/chain.toml
      - --datadir=/reth/botanix_testnet
      - --http
      - --http.addr=0.0.0.0
      - --http.port=8545
      - --http.api=admin,debug,eth,net,trace,txpool,web3,rpc
      - --http.corsdomain=*
      - -vvv
      - --bitcoind.url=http://bitcoin-core:38332
      - --bitcoind.username=${BITCOIND_USER}
      - --bitcoind.password=${BITCOIND_PASS}
      - --p2p-secret-key=/reth/botanix_testnet/discovery-secret
      - --port=30303
      - --btc-network=signet
      - --metrics=0.0.0.0:9001
      - --ipcdisable
    ports:
      - 8545:8545
      - 8546:8546
      - 9001:9001
      - 30303:30303
    depends_on:
      - bitcoin-core
    volumes:
      - ./poa-node-rpc:/reth/botanix_testnet:rw
    restart: on-failure

  alloy:
    image: grafana/alloy:latest
    command:
      - run
      - --server.http.listen-addr=0.0.0.0:12345
      - --storage.path=/var/lib/alloy/data
      - /etc/alloy/config.alloy
    ports:
      - 12345:12345
    volumes:
      - ./alloydata:/var/lib/alloy/data
      - ./alloy:/etc/alloy/
      - /etc/hostname:/etc/hostname:ro
      - /var/run/docker.sock:/var/run/docker.sock
```

### Docker Compose File Documentation

This Docker Compose file defines a multi-service setup that includes a Bitcoin Core node, a Botanix RPC node, and a Grafana Alloy instance. Below is a detailed explanation of each service.

#### 1. `bitcoin-core`

This service runs a Bitcoin Core node using the latest version of the `ruimarinho/bitcoin-core` Docker image. It operates on the Signet network for testnet.

-   **Environment Variables**: The service loads environment variables from the `.bitcoin.env` file, where `BITCOIND_USER` and `BITCOIND_PASS` are defined.
-   **Command**: The command options specify the following:
    -   `-printtoconsole`: Logs output to the console.
    -   `-signet=1`: Enables Signet mode.
    -   `-txindex=1`: Maintains a full transaction index.
    -   `-server=1`: Runs the node as a server.
    -   `-rpcport=38332`: Sets the RPC port.
    -   `-rpcuser` and `-rpcpassword`: Set the RPC authentication using environment variables.
    -   `-rpcbind=0.0.0.0` and `-rpcallowip=0.0.0.0/0`: Allow RPC connections from any IP address.
    -   `-blockfilterindex=1`: Enables block filtering.

#### 2. `poa-node-rpc`

This service runs a Botanix PoA node, which connects to the Bitcoin Core node and provides RPC (Remote Procedure Call) access.

-   **Environment Variables**: It uses the same `.bitcoin.env` file as the Bitcoin Core service.
-   **Container Name**: The container is named `botanix-poa-node-rpc`.
-   **Image**: It uses a custom Botanix image (`botanix-testnet-node-v1`).
-   **Command**: The command options specify the following
    For details on each cli flag please visit the [cli documentation
    ](../cli/poa.md)
-   **Dependencies**: This service depends on the `bitcoin-core` service to ensure it starts only after Bitcoin Core is running.
-   **Restart Policy**: The service is configured to restart on failure. The rpc node will exit if bitcoin core is not fully sync'd

**Note** To re-sync your node please remove both the database and the static file directory.

#### 3. `alloy`

This service runs Grafana Alloy, a lightweight, scalable time-series database, only used for monitoring purposes.

-   **Image**: Uses the latest `grafana/alloy` Docker image.
-   **Command**: The command options specify the following:
    -   `run`: Starts the Alloy server.
    -   `--server.http.listen-addr=0.0.0.0:12345`: Listens on all network interfaces at port 12345.
    -   `--storage.path=/var/lib/alloy/data`: Sets the storage path for data.
    -   `/etc/alloy/config.alloy`: Specifies the configuration file.

For more information please visit [rpc-compose-file](https://github.com/botanix-labs/botanix-testnet-v1-internal/tree/main)
