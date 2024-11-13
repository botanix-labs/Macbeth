# Setting up CometBFT

## Install CometBFT from Source
Full installation guidelines for CometBFT can be found on [Github](https://github.com/cometbft/cometbft/blob/main/docs/tutorials/install.md).

> **Note**
>
> You should now have the cometbft binary in build/.

## Initialize the node
To initialize nodes, run the following commands:

```env
# Node 1
cometbft init  -k "secp256k1" --home ./node1

# Node 2
cometbft init  -k "secp256k1" --home ./node2

```

> **Note**
>
> By default the output from init command is ~/.cometbft

## Update config.toml
Update ports so they don’t conflict with other peers, ie: 
```env
tcp://127.0.0.1:26657 > tcp://127.0.0.1:36657
```

Set persistent_peers like so:
```env
persistent_peers = "b29cc26a6a7157fe511f099e115c18b17d8f05c0@127.0.0.1:36656"
```

To get the peer id to use above, use these commands:
```env
cometbft show-node-id --home ./node1
cometbft show-node-id --home ./node2
```

## Update the genesis.json for all nodes
Make sure that:
1. the ``` chain_id ``` value is the same 
1. ``` max_gas ``` is set to “-1” under “consensus_params”
1. the validator pubkeys types are the same: Should be “secp256k1”
1. ``` allow_duplicate_ip ``` value is “true” if getting duplicate connections error
1. ``` addr_book_strict ``` value is “false” if running locally
1. ``` features ``` is:
    ```env
    "feature": {
                "vote_extensions_enable_height": "0",
                "pbts_enable_height": "1"
            }
    ```
1. ``` validators ``` entry has all nodes like so:
    ```env
    "validators": [
            {
                "address": "044583E6D3EFDC706BA0AE434ADA527E4E82D079",
                "pub_key": {
                    "type": "tendermint/PubKeyEd25519",
                    "value": "Ww1JVYV8bZhv49VOvJ25iLsH5Uu/Sn6J3hBOZekifVo="
                },
                "power": "10",
                "name": ""
            },
            {
                "address": "634AB30AC3F8888AA4071C893036B22BC7A3D9A7",
                "pub_key": {
                    "type": "tendermint/PubKeyEd25519",
                    "value": "hEmXgh89RUOWmvL/XU7aQWKeBsKnZWHFyTJWsbeTvlU="
                },
                "power": "10",
                "name": ""
            }
        ]
    ```

## Commands to start and reset the nodes
Start 
```env
cometbft start –home ./path_to_node
```

Reset
```env
cometbft unsafe_reset_all –home ./path_to_node
```

> **Note**
>
> ``` unsafe_reset_all ``` only resets the data directory, not the genesis.json

## Example genesis.json
```env
{
  "genesis_time": "2024-08-28T15:28:48.066686Z",
  "chain_id": "3636",
  "initial_height": "0",
  "consensus_params": {
    "block": {
      "max_bytes": "4194304",
      "max_gas": "-1"
    },
    "evidence": {
      "max_age_num_blocks": "100000",
      "max_age_duration": "172800000000000",
      "max_bytes": "1048576"
    },
    "validator": {
      "pub_key_types": ["secp256k1"]
    },
    "version": {
      "app": "0"
    },
    "synchrony": {
      "precision": "500000000",
      "message_delay": "2000000000"
    },
    "feature": {
      "vote_extensions_enable_height": "1",
      "pbts_enable_height": "0"
    }
  },
  "validators": [
    {
      "address": "009D915D631DB0A3FEFB32685779D023153698DB",
      "pub_key": {
        "type": "tendermint/PubKeySecp256k1",
        "value": "AqSCYUYNrmyoGL6j6o3cKRt63fzpdv+LB3guvIdCNziU"
      },
      "power": "10",
      "name": ""
    },
    {
      "address": "09784D2BAA503337EAB2B1B023296324BA4827A6",
      "pub_key": {
        "type": "tendermint/PubKeySecp256k1",
        "value": "A9XFfycX13DcZWZHWTSoTA8Yu6Cappw0U7Ji5hbD8l5V"
      },
      "power": "10",
      "name": ""
    }
  ],
  "app_hash": ""
}
```