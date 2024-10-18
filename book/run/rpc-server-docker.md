# Docker

Our installation docs support running RPC nodes via Docker Compose.
In the future we will provide federation member support, also via Docker Compose.

> **Note**
>
> Reth requires Docker Engine version 20.10.10 or higher due to [missing support](https://docs.docker.com/engine/release-notes/20.10/#201010) for the `clone3` syscall in previous versions.

## GitHub

Botanix Docker images are released on Docker Hub.

You can obtain the latest image with:

```bash
docker pull us-central1-docker.pkg.dev/botanix-391913/botanix-testnet-node-v1/botanix-poa-node
```

Or a specific version (e.g. v0.0.1) with:

```bash
docker pull us-central1-docker.pkg.dev/botanix-391913/botanix-testnet-node-v1/botanix-poa-node:v.0.0.1
```

### Using Docker Compose

This setup provides a environment for running a Bitcoin Core node, a Botanix RPC node, and monitoring tools using Grafana Alloy. The services are configured to work together, with appropriate dependencies and ports exposed for interaction.

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
-   **Command**: The command options are listed and explained in the [CLI documentation](../cli/poa.md)
-   **Dependencies**: This service depends on the `bitcoin-core` service to ensure it starts only after Bitcoin Core is running.
-   **Restart Policy**: The service is configured to restart on failure. The RPC node will exit if Bitcoin Core is not fully sync'd

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

### Connecting to federated testnet

Botanix will be hosting a testnet federation. To connect your RPC set up with the federation please use the following chain.toml.
Warning: this config may change in the future as we add and remove federation members.

```toml
botanix-fee-recipient = "0xb8c03cb8C9bAC79c53926E3C66344C13452105f5"

minting-contract-bytecode = "60806040526004361061003f5760003560e01c80635fe03f45146100445780636f194dc914610066578063a5d0bb93146100b3578063a8de6d8c146100d6575b600080fd5b34801561005057600080fd5b5061006461005f366004610489565b6100fd565b005b34801561007257600080fd5b50610099610081366004610512565b60006020819052908152604090205463ffffffff1681565b60405163ffffffff90911681526020015b60405180910390f35b6100c66100c1366004610534565b610349565b60405190151581526020016100aa565b3480156100e257600080fd5b506100ef6402540be40081565b6040519081526020016100aa565b60005a6001600160a01b03881660009081526020819052604090205490915063ffffffff9081169086161161018b5760405162461bcd60e51b815260206004820152602960248201527f7573657220626974636f696e426c6f636b486569676874206e6565647320746f60448201526820696e63726561736560b81b60648201526084015b60405180910390fd5b6001600160a01b0387166000908152602081905260408120805463ffffffff191663ffffffff88161790553a60016101c46004876105b6565b61048560036107d3615208805a6101db908b6105d8565b6101e591906105f1565b6101ef91906105f1565b6101f991906105f1565b61020391906105f1565b61020d91906105f1565b61021791906105f1565b61022191906105d8565b61022b9190610604565b90508681111561027d5760405162461bcd60e51b815260206004820152601c60248201527f547820636f7374206578636565647320706567696e20616d6f756e74000000006044820152606401610182565b61028781886105d8565b6040519097506001600160a01b0389169088156108fc029089906000818181858888f193505050501580156102c0573d6000803e3d6000fd5b506040516001600160a01b0384169082156108fc029083906000818181858888f193505050501580156102f7573d6000803e3d6000fd5b50876001600160a01b03167f922344dc04648c0ce028ecdf9b2c9eed9a6794dbb47b777b54b0cfe069f128aa888888886040516103379493929190610644565b60405180910390a25050505050505050565b600061035c6402540be40061014a610604565b34116103d05760405162461bcd60e51b815260206004820152603860248201527f56616c7565206d7573742062652067726561746572207468616e20647573742060448201527f616d6f756e74206f662033333020736174732f764279746500000000000000006064820152608401610182565b336001600160a01b03167f17f87987da8ca71c697791dcfd190d07630cf17bf09c65c5a59b8277d9fe17153487878787604051610411959493929190610674565b60405180910390a2506001949350505050565b80356001600160a01b038116811461043b57600080fd5b919050565b60008083601f84011261045257600080fd5b50813567ffffffffffffffff81111561046a57600080fd5b60208301915083602082850101111561048257600080fd5b9250929050565b60008060008060008060a087890312156104a257600080fd5b6104ab87610424565b955060208701359450604087013563ffffffff811681146104cb57600080fd5b9350606087013567ffffffffffffffff8111156104e757600080fd5b6104f389828a01610440565b9094509250610506905060808801610424565b90509295509295509295565b60006020828403121561052457600080fd5b61052d82610424565b9392505050565b6000806000806040858703121561054a57600080fd5b843567ffffffffffffffff8082111561056257600080fd5b61056e88838901610440565b9096509450602087013591508082111561058757600080fd5b5061059487828801610440565b95989497509550505050565b634e487b7160e01b600052601160045260246000fd5b6000826105d357634e487b7160e01b600052601260045260246000fd5b500490565b818103818111156105eb576105eb6105a0565b92915050565b808201808211156105eb576105eb6105a0565b80820281158282048414176105eb576105eb6105a0565b81835281816020850137506000828201602090810191909152601f909101601f19169091010190565b84815263ffffffff8416602082015260606040820152600061066a60608301848661061b565b9695505050505050565b85815260606020820152600061068e60608301868861061b565b82810360408401526106a181858761061b565b9897505050505050505056fea2646970667358221220cf16442b31d8d5a64fc0a5e558f76e2e76039b54484fece01be27ffcf75ede6f64736f6c63430008150033"

[[federation-member-public-key]]
key = "039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d"
socket-addr = "34.172.207.38:30303"

[[federation-member-public-key]]
key = "02bdc272b244f717604fffe659d2d98205d1e6764fdf453d1631f42c2db4d8d710"
socket-addr = "34.172.207.38:30304"

[[federation-member-public-key]]
key = "026cd057c4aa263a6c1f0c9f11045030d23e075d497ea3f359db2f8a02fcc5d52d"
socket-addr = "34.172.207.38:30305"

[[federation-member-public-key]]
key = "020c5054c4d92177a4f03ce02f6f8eeb88dd1ab49210d8f67dd0c78b40b0ffea9d"
socket-addr = "34.172.207.38:30306"

[[federation-member-public-key]]
key = "03f8635244c853f025f38539543bb8e0c29db58641593292dee03e1a670ccaa6a9"
socket-addr = "34.172.207.38:30307"

[[federation-member-public-key]]
key = "039e595a4d99b7b10d4efd052559fa0b545f83a7666a4b6a15869eb482682d7287"
socket-addr = "34.172.207.38:30308"
```
