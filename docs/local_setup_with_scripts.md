# Local Setup Guide

## Requirements

To develop and run this project locally, ensure you have the following tools installed.

1. [Rust](https://www.rust-lang.org/).
2. Binaries folder contains binary of `cometbft` & `bticoind`
3. We can find `local_setupt.sh` inside the project root, which contains commands for init, start and stop the services and nodes. 

## Project Execution Steps: 

 1.  Download binaraires `bitcoind`, `bitcoin-cli`, `bitcoin-wallet` & `cometbft` to your home directory & set 
    
     ```
     DEFAULT_BINARY_PATH="{PROVIDE_BINARIES_PATH}  //Set downloaded binary path in `local_setup.sh`
     ```
    
 2.  use below command which initialize the folders, in your home directory.

     ```
     ./local_setup.sh init 
     ```

**Note:** The `init` command will create a `temp` directory, and within that directory, it will generate subdirectories for Bitcoin nodes, POA nodes, and CometBFT nodes.

 3. Create a file named federation.toml and copy the contents of the [Federational.Toml](https://github.com/botanix-labs/Macbeth/blob/cf1d3272ace7df42e016b4dcb98bcbf1fcfd9add/book/installation/chain-config.md?plain=1#L4) file and save it node1 and node2 folders. Which contains federation members keys.
 
 4. Update the comet BFT config files, which will be created inside the comet folders.
    
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
 
 4. Use  below command to `start` a `bitcoind`, `bitcoin-server`,`POA` and `cometbft`nodes.

     ```
    ./local_setup.sh start 
     ```
 5. Use script below command to `stop` services and nodes.
    
    ```
    ./local_setup.sh stop
    ```
 6. Use script below command to `clean` services and nodes.
    
    ```
    ./local_setup.sh clean
    ```

**Note:** clean command will delete the tmp folders and contents.
