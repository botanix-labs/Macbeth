# Installation and Local Setup Guide

## Requirements

To develop and run this project locally, ensure you have the following tools installed.

1. [Rust](https://www.rust-lang.org/).
2. [Bitcoin](https://github.com/bitcoin/bitcoin).
3. [CometBFT](https://github.com/cometbft/cometbft).
4. [MacBeth](https://github.com/botanix-labs/Macbeth).

## Project Execution Steps: 
   
### SetUp Bitcoin Node & Run in Regtest Mode.
 1. Ensure you have the `bitcoind` and `bitcoin-cli` binaries either compiled from source or downloaded.
 2. We need to create a `bitcoin.conf` file, regtest configuration file. You can find the details below.  
   
 ```
   datadir=[FolderPath/Bitcoin/]
   regtest=1
   rpcuser=test123
   rpcpassword=test123
   server=1
   txindex=1
   fallbackfee=0.00001
   ```
 3. Run the bitcoin in regtest using below command. 
  ```
  ./bitcoind -conf="{folder_path}/Bitcoin/bitcoin.conf"
  ```
4. Create Wallet.
  ```
   ./bitcoin-wallet -chain=regtest -wallet=mywallet -datadir="{folder_path}/Bitcoin/" create
  ```
5. Load wallet.
  ```
  ./build/src/bitcoin-cli -rpcport=18443 -rpcuser=test123 -rpcpassword=test123 loadwallet "mywallet"
 ```
6. Mine block using bleow command.
```
 ./build/src/bitcoin-cli -rpcport=18443 -rpcuser=test123 -rpcpassword=test123 -generate 200 
```

### SetUp MacBeth Node.
 To run a local federation, we may need to operate multiple nodes, which include both a Bitcoin server-client and a reth node. Each node must be configured individually for proper operation.
 
 1. create two folder in your home directory `Node0` and `Node1` as shown below
  
 ```
  mkdir -p ~/federation/node0/ ~/federation/node1/
 ```

 2. Create a file named federation.toml and copy the contents of the [Federational.Toml]([Federational.Toml](https://github.com/botanix-labs/Macbeth/blob/cf1d3272ace7df42e016b4dcb98bcbf1fcfd9add/book/installation/chain-config.md?plain=1#L4)) file into the previously  created `federation.toml` and save it Node0 and Node1 folders.
 
 3. Create `.env` folder inside project macbeth folder and add below env config.

 ```
 cd ~/macbeth 
 ```

 ```
  BITCOIND_NETWORK=regtest
  BITCOIND_URL=http://localhost:18443
  BITCOIND_USER=test123
  BITCOIND_PWD=test123


  NODE_1_DIR= {folder_path}/federation/node0  
  NODE_2_DIR= {folder_path}/federation/node1
  
  
 ```  
 * Ensure that you provide the correct path for the `federation/node0` directory that was previously created in `NODE_1_DIR` & NODE_2_DIR, along with the Bitcoin RPC username, RPC password, and Bitcoin port.

 3. Now we need to start 2 btcoin-server-client below is command, make sure you execute below command in two separate terminal.  
 ```
   make start-btc-server-1
   make start-btc-server-2
 ```

 4. We need to start 2 reth server below is command. make sure you execute below command in two separate terminal. 
```
 make start-poa-server-1
 make start-poa-server-2
```

**Note:** Make sure 2 btc-server running properly and conncected to bitcoind regtest node &
          make sure 2 reth node is running and established the connection by participating singature sharing. 
          For more env variable details and for clearing `db`  refer macbeth `MakeFile`.


### CometBFT Node Setup
 We will be running two CometBFT nodes that will synchronize with each other via peer-to-peer communication and connect to an reth ABCI client.

1. Ensure you have the 'cometbft' binaries either compiled from source or downloaded.
2. Using below code which create 2 nodes `node0` and `node1` with config and genesis file. 
```
./build/cometbft testnet --o {directory_path} --v 2
```

3. After creating cometbft nodes, we must update cometbft config files of both node as mention below.

| **node0**                                          | **node1**                                          |
| -------------------------------------------------- | -------------------------------------------------- |
| `config.toml`                                      | `config.toml`                                      |
|                                                    |                                                    |
|   proxy_app = "tcp://127.0.0.1:26658"              |  proxy_app = "tcp://127.0.0.1:36658"               |
|   laddr = "tcp://127.0.0.1:26657"                  |  laddr = "tcp://127.0.0:36657"                     |
|   allow_duplicate_ip = true                        |  allow_duplicate_ip = true                         |
|   under[p2p] = laddr = "tcp://0.0.0.0:26656",      |  under[p2p] = laddr = "tcp://0.0.0.0:36656"        | 
|   persistent_peers = "{node_id_0}@127.0.0.1:26656, |  persistent_peers = {node_id_0}@127.0.0.1:36656"   | 
|   {node_id_1}@127.0.0.1:36656"                     |  {node_id_1}@127.0.0.1:36656"                      | 
|                                                                                                         | 
 
4. Run the two nodes using the command below and ensure that their block heights are synchronized.

```
./build/cometbft --home={{folder_path}}/node0 start 

./build/cometbft --home={folder_path}/testnet/node1 start 
```
 

**Note:** Make sure all the 6 nodes are running and producing empty blocks with proper heights.