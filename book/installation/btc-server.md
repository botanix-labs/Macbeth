# Bitcoin Signing Server

The Bitcoin signing server is responsible for managing they Bitcoin multisig keys of the federation. This service needs to be live before the poa node can begin to produce blocks.
This service only needs to be ran for block producing federation nodes.

###  Cli Refrence

```
$ cargo run -- --help 
Usage: btc-server [OPTIONS]

Options:
      --db <DB>
          The path to the database
      --config-path <CONFIG_PATH>
          The path to the database
      --btc-network <BTC_NETWORK>
          The bitcoin network to operate on
      --identifier <IDENTIFIER>
          Frost participant identifier
      --address <ADDRESS>
      
      --max-signers <MAX_SIGNERS>
          max signers
      --min-signers <MIN_SIGNERS>
          min signers
      --toml <TOML>
          toml configuration path
      --jwt-secret <JWT_SECRET>
          jwt secret path
      --bitcoind-url <BITCOIND_URL>
          bitcoind url
      --bitcoind-user <BITCOIND_USER>
          bitcoind user
      --bitcoind-pass <BITCOIND_PASS>
          bitcoind pass
      --fee-rate-diff-percentage <FEE_RATE_DIFF_PERCENTAGE>
          acceptable fee rate difference percentage as an integer (ex. 2 = 2%, 20 = 20%)
      --fall-back-fee-rate-sat-per-vbyte <FALL_BACK_FEE_RATE_SAT_PER_VBYTE>
          Fall back fee rate expressed in sat per vbyte
      --pegin-confirmation-depth <PEGIN_CONFIRMATION_DEPTH>
          The number of confirmations required for pegins
  -h, --help
          Print help


```