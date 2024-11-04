# Setting up bitcoind

To run either a Botanix RPC or Federation node you need to setup a Bitcoin block source.
Our instructions refer to bitcoind but you are free to use any bitcoin implementation.

### Getting bitcoind

Please refer to [Setting up bitcoin core](https://bitcoin.org/en/full-node)

### Base configs

The Botanix node will always use RPC credentials for authentication. Please start with these base configs.

```env
rpcuser=<username>
rpcpassword=<password>
rpcallowip=127.0.0.1
server=1
```

Note that the bitcoind RPC does not secure the traffic. It is recommended to run bitcoind on the same machine or in the same VPC as your Botanix node.

### Testnet

Botanix testnet uses bitcoin signet as its L1 chain.
To start bitcoind in signet mode please start bitcoind with the signet flag.

`signet=1`
