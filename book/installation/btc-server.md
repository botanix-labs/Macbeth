# Bitcoin Signing Server

The Bitcoin signing server is responsible for managing the Bitcoin multisig keys of the federation. This service needs to be live before the PoA (Proof of Authority) node can begin to produce blocks.
This service only needs to be ran for block producing federation nodes.

Additionally this service does not need to be publicly accessible. It is recommended that only the machine hosting your Botanix node should be able to access the Bitcoin signing server.

### Additional notes
#### What is the identifier?
Your identifier is your index into the federation list. More about this list can be found in [chain-config.md](../installation/chain-config.md).
For example if my public key's index into the list is the first one my indentifier is 0.
If its the fourth, my identifier is 3. 

#### What is the database?
This service needs to store several key pieces of information that is critical for signing bitcoin withdrawal requests.
For example the UTXO set and information about its private key share in the FROST multisig.
This database includes sensitive data and is non-recoverable once deleted.   

###  CLI reference

```bash
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