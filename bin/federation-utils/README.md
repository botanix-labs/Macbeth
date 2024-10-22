# Utils CLI

A command-line interface (CLI) for interacting with a blockchain network. It provides tools for fetching balance and sweeping balance to another address.

## Usage

```bash
Usage: utils [OPTIONS] <COMMAND>

Commands:
  init           Init create config.toml in home directory
  get-balance    Get balance of an address
  sweep-balance  Sweep the balance to another address
  help           Print this message or the help of the given subcommand(s)

Options:
  -p, --config-path <CONFIG_PATH>    Config.toml path (default path is home directory)
  -c, --chain-id <CHAIN_ID>          Chain ID (default 3636, can be loaded from `config.toml`)
  -u, --provider-url <PROVIDER_URL>  Provider URL (defaults to `http://localhost:8545`, can be loaded from `config.toml`)
  -h, --help                         Print help
```

## Build

 ```
  cd bin/federation-utils
  cargo build

 ```
 ## Usage 

 ```bash
 ./target/debug/utils   --config-path {path_config.toml} init
```

### GetBlance :

* Using  config toml
```bash
 ./target/debug/utils  --config-path {path_config.toml} get-balance 
```
* CLI Command
```bash
  ../../target/debug/utils --chain-id 36363 --provider-url http://localhost:8545  get-balance -s <SECRET_KEY_PATH>
```

### SweepBalance :

* Using config toml
```bash
 ./target/debug/utils --config-path {path_config.toml} sweep-balance
```

* CLI Command
```bash
  ../target/debug/utils --chain-id 36363 --provider-url http://localhost:8545  sweep-balance --secret-key-path <SECRET_KEY_PATH> --receiver-address <RECEIVER_ADDRESS>
```

### GetTransactionInfo :

* CLI Command
```bash
  ../target/debug/utils --chain-id 36363 --provider-url http://localhost:8545 get-transaction <tx-hash>
```

### Config.toml  
```
chain_id = 3636
provider_url = "http://localhost:8545"
secret_path = "<path-to-secret>"    //private_key path provided by user for sweep-balance, only support Secp256k1.
receiver_address = "<enter-receiver-address>" //receiver_address is used in sweep-balance command,transfer token to receive balance.
```

**Note:** Values can be provided either directly through the CLI command or specified within the `config.toml` file for automatic configuration. This flexibility allows you to set parameters according to your preference.