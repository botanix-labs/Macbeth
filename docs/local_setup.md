# Installation and Local Setup Guide

## Requirements

To develop and run this project locally, ensure you have the following tools installed.

1. [Rust](https://www.rust-lang.org/).
2. [Bitcoin](https://github.com/bitcoin/bitcoin).
3. [CometBFT](https://github.com/cometbft/cometbft).

## Project Execution Steps

### Setup Bitcoin Node & Run in Regtest Mode

1.  Ensure you have the `bitcoind` and `bitcoin-cli` binaries either compiled from source or downloaded.
2.  We need to create a `bitcoin.conf` file, regtest configuration file. You can find the details below.

```
# Use regtest as default
regtest=1

# Options for mainnet
[main]

# Options for testnet
[test]

# Options for signet
[signet]

# Options for regtest
[regtest]
datadir=/path/to/data/
rpcuser=test123
rpcpassword=test123
server=1
txindex=1
fallbackfee=0.00001
rpcallowip=127.0.0.1
zmqpubrawblock=tcp://127.0.0.1:28332
zmqpubrawtx=tcp://127.0.0.1:28333
```

3.  Run the bitcoin in regtest using below command.

```
bitcoind -conf="/path/to/data/bitcoin.conf"
```

4. Create Wallet.

```
bitcoin-wallet -chain=regtest -wallet=mywallet -datadir="/path/to/data/" create
```

5. Load wallet.

```
bitcoin-cli -rpcport=18443 -rpcuser=test123 -rpcpassword=test123 loadwallet "mywallet"
```

6. Mine block using bleow command.

```
bitcoin-cli -rpcport=18443 -rpcuser=test123 -rpcpassword=test123 -generate 200
```

### Setup MacBeth Node

To run a local federation, we may need to operate multiple nodes, which include both a bitcoin server and a reth node. Each node must be configured individually for proper operation.Please note that federation on reth node consists of at least two federation members.

1.  create two folder in your home directory `Node0` and `Node1` as shown below

```
mkdir -p /path/to/federation/node0/ /path/to/federation/node1/
```

2.  Create a file named federation.toml and copy the contents of the [federation.toml](https://github.com/botanix-labs/Macbeth/blob/cf1d3272ace7df42e016b4dcb98bcbf1fcfd9add/book/installation/chain-config.md?plain=1#L4) file and save it in the `node0` and `node1` folders. Which contains federation members keys.

    -   For **testing purposes**, you can use a predefined federation. Be aware that this setup uses **publicly exposed private keys**, which should not be used in a production environment.

        ```
        cp -R docs/sample-federation/* /path/to/federation # WARNING: leaked secret keys, use for testing only!
        ```

3.  Create `.env` inside project macbeth folder and add below env config.

```
BITCOIND_NETWORK=regtest
BITCOIND_URL=http://127.0.0.1:18443
BITCOIND_USER=test123
BITCOIND_PWD=test123

NODE_1_DIR=/path/to/federation/node0
NODE_2_DIR=/path/to/federation/node1
```

4.  Now we need to start 2 btcoin-server-client below is command, make sure you execute below command in two separate terminal.

```
make start-btc-server-1
make start-btc-server-2
```

5.  We need to start 2 reth server below is command. make sure you execute below command in two separate terminal.

```
make start-poa-server-1
make start-poa-server-2
```

**Note:** Make sure 2 btc-server running properly and conncected to bitcoind regtest node &
make sure 2 reth node is running and established the connection by participating signature sharing.
For more env variable details and for clearing `db` refer macbeth `MakeFile`.

### Setup CometBFT Node

We will be running two CometBFT nodes that will synchronize with each other via peer-to-peer communication and connect to an reth ABCI client.

1. Ensure you have the 'cometbft' binaries either compiled from source or downloaded.
2. Using below code which create 2 nodes `node0` and `node1` with config and genesis file.

```
cometbft testnet --o /path/to/consensus --v 2
```

3. After creating cometbft nodes, we must update cometbft config files of both node as mention below:

-   `/path/to/consensus/node0/config/config.toml`
-   `/path/to/consensus/node1/config/config.toml`

| **node0**                                        | **node1**                                       |
| ------------------------------------------------ | ----------------------------------------------- |
| `config.toml`                                    | `config.toml`                                   |
|                                                  |                                                 |
| proxy_app = "tcp://127.0.0.1:26658"              | proxy_app = "tcp://127.0.0.1:36658"             |
| [rpc].laddr = "tcp://127.0.0.1:26657"            | [rpc].laddr = "tcp://127.0.0:36657"             |
| allow_duplicate_ip = true                        | allow_duplicate_ip = true                       |
| [p2p].laddr = "tcp://0.0.0.0:26656"              | [p2p].laddr = "tcp://0.0.0.0:36656"             |
| persistent_peers = "{node_id_0}@127.0.0.1:26656, | persistent_peers = "{node_id_0}@127.0.0.1:36656 |
| {node_id_1}@127.0.0.1:36656"                     | {node_id_1}@127.0.0.1:36656"                    |
|                                                  |                                                 |

For **testing purposes**, you can use a predefined cometbft configuration. Be aware that this setup uses **publicly exposed private keys**, which should not be used in a production environment.

```
cp -R docs/sample-cometbft/* /path/to/consensus # WARNING: leaked secret keys, use for testing only!
```

4. Run the two nodes using the command below and ensure that their block heights are synchronized.

```
cometbft --home=/path/to/consensus/node0 start
cometbft --home=/path/to/consensus/node1 start
```

**Note:** Make sure all the 6 nodes with bitcond in regtest are running and producing empty blocks with proper heights.
