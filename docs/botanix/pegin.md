

Pegin procedure
===============

# Participants

* Alice: user that wants to peg in
* SideCar: some kind of dapp server or Botanix server that helps users with pegins
  and has access to an index of the bitcoin chain (like an esplora or so)
* Minting contract on Botanix chain
* botanix chain validators: anyone running a botanix chain in validating mode (is that a thing?)


# Steps

1. Alice creates her **ethereum public key** and a **pegin nonce** which can either be an increasing
   counter or just the current time in milliseconds. Current time will be simpler, as it wouldn't require application state.

1. Alice signs her nonce to create a **nonceCommitment** by signing the nonce + pegin tag. `nonceCommitment = H( "botanix-pegin::" | nonce)`

1. Alice sends `[nonce, nonceCommitment, ethAddrress]` to SideCar. SideCar will query the Botanix network via RPC to get the current aggregated public key i.e FROST pubkey.

1. SideCar will send all neccecary components for a GA to Botanix via RPC to get a Gateway address. Note that the RPC to get a GA is purely a utility that abstracts away the complexity of generating the taproot address. To verify, SideCar can generate the same taproot address. Additinally this RPC node should be authenticated or rate-limited. Without a rate-limiting method this utlity method is subject to spam attack.

1. Botanix Protocol will  combines this info with the FROST pubkey to create the internal key for her taproot
    gateway address: `I = FROST + H(FROST | ethAddress | nonce) * G`. The taproot would then be
    calculated using the taproot equation `Q = I + TapTweak(I | S) * G`.
    And she sends her pegin transaction to that taproot address on the Bitcoin chain. Additional tapscripts will include the safe spend path. More to come on that in a different spec.

1. Alice sends her sats to her gateway address.

1. Alice constructs a pegin tx which calls the mint method in the mint contract. Signs the tx and delivers it to SideCar.

1. After the bitcoin transaction reaches 6 confirmations, the SideCar takes the transaction
   and constructs the pegin proof by combining the tx, the block it was confirmed (merkle proof) and
   the most recent block headers between the block the tx got confirmed in and the tip.

1. The SideCar generates a Botanix pegin tx calling into the Minting contract providing Alice's
   information and the pegin proof. SideCar will manage its own Botanix Eth account and provide the gas for the pegin tx.

   NOTE: It's important to note that the above two steps could be done by Alice, but the helper
   just makes it so that Alice doesn't have to construct her pegin proof, doesn't need a Bitcoin node
   and doesn't need to know how to call into the Minting contract. Advanced peginners might want to
   do this themselves, but in this simple UX flow, some stateless helper can easily do it.

1. The contract will check if the pegin increments Alice's pegin nonce and emit a `Mint` event
   containing the pegin data on success. It will also mint the amount to Alice.

1. Any chain validator, both when applying the tx to mempool or when validating a block, monitors for 
   events of this type from this contract and performs a check to validate the proof.
   This can happen outside of EVM execution, probably right after a tx or block executions summary is
   communicated to some upstream layer.


NOTE: Calls into the `Minting` contract are supposed to somehow be free of gas, but are required to
pass these additional consensus checks.

The Botanix pegin proof will be structured as follows:
| Field                  | Description                   | Size  |
|------------------------|-------------------------------|-------|
| Pegin Message Version  |                               | 4 byte|
| txId  |                               | 4 byte|
| Vout  |                               | 4 byte|
|  Ethereum Address |                               | 20 bytes|
|  Aggregate Public Key| Compressed public key                              | 33 bytes|
| Number of Block | Bitcoin style var int | 1-3 bytes|
| Headers                |  Variable number of 80-byte blocks headers  |   80 * num of blocks    |
| ** The rest of the payload is the merkle inclusion proof ** |   |   |
| Number of transactions repersented by merkle root | Uint32       | 4 byte|
| Number of hashes | Bitcoin style var int | 1-3 bytes|
| Merkle Hashes | hash pairings | 32 bytes * number of hashes|
| Number of bytes of Inclusion bits | Bitcoin style var int | 1-3 bytes|
| Inclusion bits |  | num of transaction / 8 bytes |

