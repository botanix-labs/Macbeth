# `reth poa`

Initialize the Botanix PoA node

```bash
$ reth poa --help
Start the POA node

Usage: reth poa [OPTIONS]

Options:
      --datadir <DATA_DIR>
          The path to the data dir for all reth files and subdirectories.

          Defaults to the OS-specific data directory:

          - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
          - macOS: `$HOME/Library/Application Support/reth/`

          [default: default]

      --network-config-path <FILE>
          The path to the configuration file to use for network properties.

      --chain <CHAIN_OR_PATH>
          The chain this node is running.
          Possible values are either a built-in chain or the path to a chain specification file.

          Built-in chains:
              mainnet, sepolia, goerli, holesky, dev, botanix_testnet

          [default: mainnet]

      --federation-mode
          Run in federation mode. Only the nodes in the federation will be able to produce blocks

      --instance <INSTANCE>
          Add a new instance of a node.

          Configures the ports of the node to avoid conflicts with the defaults. This is useful for running multiple nodes on the same machine.

          Max number of instances is 200. It is chosen in a way so that it is not possible to have port numbers that conflict with each other.

          Changes to the following port numbers: - DISCOVERY_PORT: default + `instance` - 1 - AUTH_PORT: default + `instance` * 100 - 100 - HTTP_RPC_PORT: default - `instance` + 1 - WS_RPC_PORT: default + `instance` * 2 - 2

          [default: 1]

      --with-unused-ports
          Sets all ports to unused, allowing the OS to choose random unused ports when sockets are bound.

          Mutually exclusive with `--instance`.

  -h, --help
          Print help (see a summary with '-h')

Metrics:
      --metrics <SOCKET>
          Enable Prometheus metrics.

          The metrics will be served at the given interface and port.

Abci client/app:
      --abci-host
          [default: 0.0.0.0]
      --abci-port
          [default: 26658]

Networking:
  -d, --disable-discovery
          Disable the discovery service

      --disable-dns-discovery
          Disable the DNS discovery

      --disable-discv4-discovery
          Disable Discv4 discovery

      --enable-discv5-discovery
          Enable Discv5 discovery

      --discovery.addr <DISCOVERY_ADDR>
          The UDP address to use for devp2p peer discovery version 4

          [default: 0.0.0.0]

      --discovery.port <DISCOVERY_PORT>
          The UDP port to use for devp2p peer discovery version 4

          [default: 30303]

      --discovery.v5.addr <DISCOVERY_V5_ADDR>
          The UDP address to use for devp2p peer discovery version 5

          [default: 0.0.0.0]

      --discovery.v5.port <DISCOVERY_V5_PORT>
          The UDP port to use for devp2p peer discovery version 5

          [default: 9000]

      --trusted-peers <TRUSTED_PEERS>
          Comma separated enode URLs of trusted peers for P2P connections.

          --trusted-peers enode://abcd@192.168.0.1:30303

      --trusted-only
          Connect only to trusted peers

      --bootnodes <BOOTNODES>
          Comma separated enode URLs for P2P discovery bootstrap.

          Will fall back to a network-specific default if not specified.

      --peers-file <FILE>
          The path to the known peers file. Connected peers are dumped to this file on nodes
          shutdown, and read on startup. Cannot be used with `--no-persist-peers`.

      --identity <IDENTITY>
          Custom node identity

          [default: reth/v0.2.0-beta.6-778feb0a2/x86_64-apple-darwin]

      --p2p-secret-key <PATH>
          Secret key to use for this node.

          This will also deterministically set the peer ID. If not specified, it will be set in the data dir for the chain being used.

      --no-persist-peers
          Do not persist peers.

      --nat <NAT>
          NAT resolution method (any|none|upnp|publicip|extip:\<IP\>)

          [default: any]

      --addr <ADDR>
          Network listening address

          [default: 0.0.0.0]

      --port <PORT>
          Network listening port

          [default: 30303]

      --max-outbound-peers <MAX_OUTBOUND_PEERS>
          Maximum number of outbound requests. default: 100

      --max-inbound-peers <MAX_INBOUND_PEERS>
          Maximum number of inbound requests. default: 30

      --pooled-tx-response-soft-limit <BYTES>
          Soft limit for the byte size of a `PooledTransactions` response on assembling a `GetPooledTransactions` request. Spec`d at 2 MiB.

          <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#protocol-messages>.

          [default: 2097152]

      --pooled-tx-pack-soft-limit <BYTES>
          Default soft limit for the byte size of a `PooledTransactions` response on assembling a `GetPooledTransactions` request. This defaults to less than the [`SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE`], at 2 MiB, used when assembling a `PooledTransactions` response. Default is 128 KiB

          [default: 131072]

RPC:
      --http
          Enable the HTTP-RPC server

      --http.addr <HTTP_ADDR>
          Http server address to listen on

          [default: 127.0.0.1]

      --http.port <HTTP_PORT>
          Http server port to listen on

          [default: 8545]

      --http.api <HTTP_API>
          Rpc Modules to be configured for the HTTP server

          [possible values: admin, debug, eth, net, trace, txpool, web3, rpc, reth, ots, eth-call-bundle]

      --http.corsdomain <HTTP_CORSDOMAIN>
          Http Corsdomain to allow request from

      --ws
          Enable the WS-RPC server

      --ws.addr <WS_ADDR>
          Ws server address to listen on

          [default: 127.0.0.1]

      --ws.port <WS_PORT>
          Ws server port to listen on

          [default: 8546]

      --ws.origins <ws.origins>
          Origins from which to accept WebSocket requests

      --ws.api <WS_API>
          Rpc Modules to be configured for the WS server

          [possible values: admin, debug, eth, net, trace, txpool, web3, rpc, reth, ots, eth-call-bundle]

      --ipcdisable
          Disable the IPC-RPC server

      --ipcpath <IPCPATH>
          Filename for IPC socket/pipe within the datadir

          [default: /tmp/reth.ipc]

      --authrpc.addr <AUTH_ADDR>
          Auth server address to listen on

          [default: 127.0.0.1]

      --authrpc.port <AUTH_PORT>
          Auth server port to listen on

          [default: 8551]

      --authrpc.jwtsecret <PATH>
          Path to a JWT secret to use for the authenticated engine-API RPC server.

          This will enforce JWT authentication for all requests coming from the consensus layer.

          If no path is provided, a secret will be generated and stored in the datadir under `<DIR>/<CHAIN_ID>/jwt.hex`. For mainnet this would be `~/.reth/mainnet/jwt.hex` by default.

      --auth-ipc
          Enable auth engine API over IPC

      --auth-ipc.path <AUTH_IPC_PATH>
          Filename for auth IPC socket/pipe within the datadir

          [default: /tmp/reth_engine_api.ipc]

      --rpc.jwtsecret <HEX>
          Hex encoded JWT secret to authenticate the regular RPC server(s), see `--http.api` and `--ws.api`.

          This is __not__ used for the authenticated engine-API RPC server, see `--authrpc.jwtsecret`.

      --rpc.max-request-size <RPC_MAX_REQUEST_SIZE>
          Set the maximum RPC request payload size for both HTTP and WS in megabytes

          [default: 15]

      --rpc.max-response-size <RPC_MAX_RESPONSE_SIZE>
          Set the maximum RPC response payload size for both HTTP and WS in megabytes

          [default: 160]
          [aliases: rpc.returndata.limit]

      --rpc.max-subscriptions-per-connection <RPC_MAX_SUBSCRIPTIONS_PER_CONNECTION>
          Set the maximum concurrent subscriptions per connection

          [default: 1024]

      --rpc.max-connections <COUNT>
          Maximum number of RPC server connections

          [default: 500]

      --rpc.max-tracing-requests <COUNT>
          Maximum number of concurrent tracing requests

          [default: 10]

      --rpc.max-blocks-per-filter <COUNT>
          Maximum number of blocks that could be scanned per filter request. (0 = entire chain)

          [default: 100000]

      --rpc.max-logs-per-response <COUNT>
          Maximum number of logs that can be returned in a single response. (0 = no limit)

          [default: 20000]

      --rpc.gascap <GAS_CAP>
          Maximum gas limit for `eth_call` and call tracing RPC methods

          [default: 50000000]

RPC State Cache:
      --rpc-cache.max-blocks <MAX_BLOCKS>
          Max number of blocks in cache

          [default: 5000]

      --rpc-cache.max-receipts <MAX_RECEIPTS>
          Max number receipts in cache

          [default: 2000]

      --rpc-cache.max-envs <MAX_ENVS>
          Max number of bytes for cached env data

          [default: 1000]

      --rpc-cache.max-concurrent-db-requests <MAX_CONCURRENT_DB_REQUESTS>
          Max number of concurrent database requests

          [default: 512]

Gas Price Oracle:
      --gpo.blocks <BLOCKS>
          Number of recent blocks to check for gas price

          [default: 20]

      --gpo.ignoreprice <IGNORE_PRICE>
          Gas Price below which gpo will ignore transactions

          [default: 2]

      --gpo.maxprice <MAX_PRICE>
          Maximum transaction priority fee(or gasprice before London Fork) to be recommended by gpo

          [default: 500000000000]

      --gpo.percentile <PERCENTILE>
          The percentile of gas prices to use for the estimate

          [default: 60]

Btc_server:
      --btc-server <BTC_SERVER>
          Btc signing service

          The metrics will be served at the given interface and port.

Bitcoind:
      --bitcoind.url <BITCOIND_URL>
          bitcoind RPC url

          The url of the bitcoind server.

          [default: localhost:18443]

      --bitcoind.username <BITCOIND_USERNAME>
          Btcd username

          The username of the bitcoind server.

          [default: foo]

      --bitcoind.password <BITCOIND_PASSWORD>
          Btcd password

          The password of the bitcoind server.

          [default: bar]

      --frost.min_signers <MIN_SIGNERS>
          The minimum number required for frost signing

      --frost.max_signers <MAX_SIGNERS>
          The maximum number required for frost signing

Btc_network:
      --btc-network <BITCOIN_NETWORK>
          The bitcoin network to operate on

          [default: regtest]

TxPool:
      --txpool.pending-max-count <PENDING_MAX_COUNT>
          Max number of transaction in the pending sub-pool

          [default: 10000]

      --txpool.pending-max-size <PENDING_MAX_SIZE>
          Max size of the pending sub-pool in megabytes

          [default: 20]

      --txpool.basefee-max-count <BASEFEE_MAX_COUNT>
          Max number of transaction in the basefee sub-pool

          [default: 10000]

      --txpool.basefee-max-size <BASEFEE_MAX_SIZE>
          Max size of the basefee sub-pool in megabytes

          [default: 20]

      --txpool.queued-max-count <QUEUED_MAX_COUNT>
          Max number of transaction in the queued sub-pool

          [default: 10000]

      --txpool.queued-max-size <QUEUED_MAX_SIZE>
          Max size of the queued sub-pool in megabytes

          [default: 20]

      --txpool.max-account-slots <MAX_ACCOUNT_SLOTS>
          Max number of executable transaction slots guaranteed per account

          [default: 16]

      --txpool.pricebump <PRICE_BUMP>
          Price bump (in %) for the transaction pool underpriced check

          [default: 10]

      --blobpool.pricebump <BLOB_TRANSACTION_PRICE_BUMP>
          Price bump percentage to replace an already existing blob transaction

          [default: 100]

      --txpool.max-tx-input-bytes <MAX_TX_INPUT_BYTES>
          Max size in bytes of a single transaction allowed to enter the pool

          [default: 131072]

      --txpool.max-cached-entries <MAX_CACHED_ENTRIES>
          The maximum number of blobs to keep in the in memory blob cache

          [default: 100]

      --txpool.nolocals
          Flag to disable local transaction exemptions

      --txpool.locals <LOCALS>
          Flag to allow certain addresses as local

      --txpool.no-local-transactions-propagation
          Flag to toggle local transaction propagation

Debug:
      --debug.continuous
          Prompt the downloader to download blocks one at a time.

          NOTE: This is for testing purposes only.

      --debug.terminate
          Flag indicating whether the node should be terminated after the pipeline sync

      --debug.tip <TIP>
          Set the chain tip manually for testing purposes.

          NOTE: This is a temporary flag

      --debug.max-block <MAX_BLOCK>
          Runs the sync only up to the specified block

      --debug.print-inspector
          Print opcode level traces directly to console during execution

      --debug.hook-block <HOOK_BLOCK>
          Hook on a specific block during execution

      --debug.hook-transaction <HOOK_TRANSACTION>
          Hook on a specific transaction during execution

      --debug.hook-all
          Hook on every transaction in a block

      --debug.skip-fcu <SKIP_FCU>
          If provided, the engine will skip `n` consecutive FCUs

      --debug.engine-api-store <PATH>
          The path to store engine API messages at. If specified, all of the intercepted engine API messages will be written to specified location

Database:
      --db.log-level <LOG_LEVEL>
          Database logging level. Levels higher than "notice" require a debug build

          Possible values:
          - fatal:   Enables logging for critical conditions, i.e. assertion failures
          - error:   Enables logging for error conditions
          - warn:    Enables logging for warning conditions
          - notice:  Enables logging for normal but significant condition
          - verbose: Enables logging for verbose informational
          - debug:   Enables logging for debug-level messages
          - trace:   Enables logging for trace debug-level messages
          - extra:   Enables logging for extra debug-level messages

      --db.exclusive <EXCLUSIVE>
          Open environment in exclusive/monopolistic mode. Makes it possible to open a database on an NFS volume

          [possible values: true, false]

      --bitcoind-config-path <FILE>
          The path to the configuration file to use for network properties.

Logging:
      --log.stdout.format <FORMAT>
          The format to use for logs written to stdout

          [default: terminal]

          Possible values:
          - json:     Represents JSON formatting for logs. This format outputs log records as JSON objects, making it suitable for structured logging
          - log-fmt:  Represents logfmt (key=value) formatting for logs. This format is concise and human-readable, typically used in command-line applications
          - terminal: Represents terminal-friendly formatting for logs

      --log.stdout.filter <FILTER>
          The filter to use for logs written to stdout

          [default: ]

      --log.file.format <FORMAT>
          The format to use for logs written to the log file

          [default: terminal]

          Possible values:
          - json:     Represents JSON formatting for logs. This format outputs log records as JSON objects, making it suitable for structured logging
          - log-fmt:  Represents logfmt (key=value) formatting for logs. This format is concise and human-readable, typically used in command-line applications
          - terminal: Represents terminal-friendly formatting for logs

      --log.file.filter <FILTER>
          The filter to use for logs written to the log file

          [default: debug]

      --log.file.directory <PATH>
          The path to put log files in

          [default: /Users/armins/Library/Caches/reth/logs]

      --log.file.max-size <SIZE>
          The maximum size (in MB) of one log file

          [default: 200]

      --log.file.max-files <COUNT>
          The maximum amount of log files that will be stored. If set to 0, background file logging is disabled

          [default: 5]

      --log.journald
          Write logs to journald

      --log.journald.filter <FILTER>
          The filter to use for logs written to journald

          [default: error]

      --color <COLOR>
          Sets whether or not the formatter emits ANSI terminal escape codes for colors and other text formatting

          [default: always]

          Possible values:
          - always: Colors on
          - auto:   Colors on
          - never:  Colors off

Display:
  -v, --verbosity...
          Set the minimum log level.

          -v      Errors
          -vv     Warnings
          -vvv    Info
          -vvvv   Debug
          -vvvvv  Traces (warning: very verbose!)

  -q, --quiet
          Silence all log output



```
