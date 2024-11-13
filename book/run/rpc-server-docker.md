# Docker

Our installation docs support running RPC nodes via Docker Compose.
In the future we will provide federation member support, also via Docker Compose.

> **Note**
>
> Reth requires Docker Engine version 20.10.10 or higher due to [missing support](https://docs.docker.com/engine/release-notes/20.10/#201010) for the `clone3` syscall in previous versions.

## Prerequisites
To use the instructions below, you’ll need to run mutinynet signet node. Ensure that this node is fully synced to the tip before proceeding with the Docker Compose instructions provided afterward. You can find instructions for running your own node at the following links:
 - [https://github.com/benthecarman/bitcoin/releases](https://github.com/benthecarman/bitcoin/releases)
 - [https://github.com/MutinyWallet/mutiny-net](https://github.com/MutinyWallet/mutiny-net)

 > **Note**
 >
 > Mutinynet is a fork of bitcoin core that is configured for 30 second blocks. This allows our team to test more rapidly. There is a whole suite of tools available for MutinyNet, including [coin faucet](https://faucet.mutinynet.com) and [block explorer](https://mutinynet.com).
 

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
  poa-node-rpc:
    env_file:
      - .bitcoin.env
    container_name: poa-node-rpc
    image: us-central1-docker.pkg.dev/botanix-391913/botanix-testnet-node-v1/botanix-poa-node
    command:
      - poa
      - --federation-config-path=/reth/botanix_testnet/chain.toml
      - --datadir=/reth/botanix_testnet
      - --http
      - --http.addr=0.0.0.0
      - --http.port=8545
      - --http.api=debug,eth,net,trace,txpool,web3,rpc
      - --http.corsdomain=*
      - --ws
      - --ws.addr=0.0.0.0
      - --ws.port=8546
      - -vvv
      - --bitcoind.url=${BITCOIND_HOST}
      - --bitcoind.username=${BITCOIND_USER}
      - --bitcoind.password=${BITCOIND_PASS}
      - --p2p-secret-key=/reth/botanix_testnet/discovery-secret
      - --port=30303
      - --btc-network=signet
      - --metrics=0.0.0.0:9001
      - --ipcdisable
      - --abci-port=26658
      - --abci-host=0.0.0.0
      - --cometbft-rpc-port=8888
      - --cometbft-rpc-host=consensus-node
    ports:
      - 8545:8545
      - 8546:8546
      - 9001:9001
      - 30303:30303
      - 26658:26658
      - 8888:8888
    volumes:
      - ./poa-rpc:/reth/botanix_testnet:rw
    restart: on-failure

  consensus-node:
    container_name: consensus-node
    image: us-central1-docker.pkg.dev/botanix-391913/botanix-testnet-cometbft/botanix-testnet-cometft:v4
    ports:
        - 26656:26656
        - 26657:26657
        - 26660:26660
    volumes:
        - ./consensus-node:/cometbft:rw
    restart: on-failure
    environment:
        - ALLOW_DUPLICATE_IP=TRUE
        - LOG_LEVEL=DEBUG
        - NODE_NAME=poa-node-rpc
        - MONIKER=botanix-consensus-node
        - PERSISTENT_PEERS=2561602572b54dbdcf44b02157ab62717c09d895@34.35.52.165:26656, dbd6bec8f89ec52232280d92f5b67069c5344095@35.201.136.224:26656, 45aabbb31b04257a86172e7002d25b2e923b896c@34.79.189.111:26656
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


For more information please visit [rpc-compose-file](https://github.com/botanix-labs/botanix-testnet-v1-internal/tree/main)

### Connecting to federated testnet

Botanix will be hosting a testnet federation. To connect your RPC set up with the federation please use the following chain.toml.
Warning: this config may change in the future as we add and remove federation members.

```toml
botanix-fee-recipient="0xb8c03cb8C9bAC79c53926E3C66344C13452105f5"

minting-contract-bytecode = "60806040526004361061003f5760003560e01c80635fe03f45146100445780636f194dc914610066578063a5d0bb93146100b3578063a8de6d8c146100d6575b600080fd5b34801561005057600080fd5b5061006461005f366004610562565b6100fd565b005b34801561007257600080fd5b506100996100813660046105eb565b60006020819052908152604090205463ffffffff1681565b60405163ffffffff90911681526020015b60405180910390f35b6100c66100c136600461060d565b610422565b60405190151581526020016100aa565b3480156100e257600080fd5b506100ef6402540be40081565b6040519081526020016100aa565b60005a6001600160a01b03881660009081526020819052604090205490915063ffffffff9081169086161161018b5760405162461bcd60e51b815260206004820152602960248201527f7573657220626974636f696e426c6f636b486569676874206e6565647320746f60448201526820696e63726561736560b81b60648201526084015b60405180910390fd5b6001600160a01b0387166000908152602081905260408120805463ffffffff191663ffffffff88161790553a60016101c460048761068f565b61048560036107d36108fc805a6101db908b6106b1565b6101e591906106c8565b6101ef91906106c8565b6101f991906106c8565b61020391906106c8565b61020d91906106c8565b61021791906106c8565b61022191906106b1565b61022b91906106e0565b90508681111561027d5760405162461bcd60e51b815260206004820152601c60248201527f547820636f7374206578636565647320706567696e20616d6f756e74000000006044820152606401610182565b61028781886106b1565b96506000886001600160a01b03168860405160006040518083038185875af1925050503d80600081146102d6576040519150601f19603f3d011682016040523d82523d6000602084013e6102db565b606091505b505090508061032c5760405162461bcd60e51b815260206004820152601a60248201527f4d696e7420746f2064657374696e6174696f6e206661696c65640000000000006044820152606401610182565b6000846001600160a01b03168360405160006040518083038185875af1925050503d8060008114610379576040519150601f19603f3d011682016040523d82523d6000602084013e61037e565b606091505b50509050806103cf5760405162461bcd60e51b815260206004820152601e60248201527f526566756e6420746f20726566756e6441646472657373206661696c656400006044820152606401610182565b896001600160a01b03167f922344dc04648c0ce028ecdf9b2c9eed9a6794dbb47b777b54b0cfe069f128aa8a8a8a8a60405161040e9493929190610728565b60405180910390a250505050505050505050565b60006104356402540be40061014a6106e0565b34116104a95760405162461bcd60e51b815260206004820152603860248201527f56616c7565206d7573742062652067726561746572207468616e20647573742060448201527f616d6f756e74206f662033333020736174732f764279746500000000000000006064820152608401610182565b336001600160a01b03167f17f87987da8ca71c697791dcfd190d07630cf17bf09c65c5a59b8277d9fe171534878787876040516104ea959493929190610758565b60405180910390a2506001949350505050565b80356001600160a01b038116811461051457600080fd5b919050565b60008083601f84011261052b57600080fd5b50813567ffffffffffffffff81111561054357600080fd5b60208301915083602082850101111561055b57600080fd5b9250929050565b60008060008060008060a0878903121561057b57600080fd5b610584876104fd565b955060208701359450604087013563ffffffff811681146105a457600080fd5b9350606087013567ffffffffffffffff8111156105c057600080fd5b6105cc89828a01610519565b90945092506105df9050608088016104fd565b90509295509295509295565b6000602082840312156105fd57600080fd5b610606826104fd565b9392505050565b6000806000806040858703121561062357600080fd5b843567ffffffffffffffff8082111561063b57600080fd5b61064788838901610519565b9096509450602087013591508082111561066057600080fd5b5061066d87828801610519565b95989497509550505050565b634e487b7160e01b600052601160045260246000fd5b6000826106ac57634e487b7160e01b600052601260045260246000fd5b500490565b6000828210156106c3576106c3610679565b500390565b600082198211156106db576106db610679565b500190565b60008160001904831182151516156106fa576106fa610679565b500290565b81835281816020850137506000828201602090810191909152601f909101601f19169091010190565b84815263ffffffff8416602082015260606040820152600061074e6060830184866106ff565b9695505050505050565b8581526060602082015260006107726060830186886106ff565b82810360408401526107858185876106ff565b9897505050505050505056fea264697066735822122058bba5f85cc573a5323f630452faca186769309f0808e1ca3fdf25351f8d078264736f6c634300080d0033"

# >>>>>>>>>>> federation members public keys
[[federation-member-public-key]]
key="039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d"
socket-addr="34.79.189.111:30303"

[[federation-member-public-key]]
key="02bdc272b244f717604fffe659d2d98205d1e6764fdf453d1631f42c2db4d8d710"
socket-addr="34.35.52.165:30303"

[[federation-member-public-key]]
key="0234324e2ef7a3c4a27884d939d2ef2138e309aa7538915ae77137d0f792881be8"
socket-addr="35.201.136.224:30303"
```
