

# proof-of-authority consensus protocol

The purpose of this document is to outline how the botanix protocol is going to maintain a its list of federation memebers. And how that list can be updated via a voting mechanism. There is a releated goal of using the same list to aquired the aggregated public key used in a FROST signing group.


### Abstract

Proof of Authority (PoA) is a consensus algorithm used in blockchain networks. In a PoA system, a group of authorized validators, often referred to as authorities, are responsible for validating and adding new blocks to the blockchain. These authorities are trusted and known entities, typically organizations or individuals with a strong reputation.

### To contract or not

This is a major design contraint and should be thought about carefully.
>A PoA scheme is based on the idea that blocks may only be minted by trusted signers. As such, every block (or header) that a client sees can be matched against the list of trusted signers. The challenge here is how to maintain a list of authorized signers that can change in time? The obvious answer (store it in an Ethereum contract) is also the wrong answer: fast, light and warp sync don’t have access to the state during syncing.

The major flaws with storing authority identities in a contract is inassible data during sync and block validatinon.

As such, [Clique POA consensus](https://eips.ethereum.org/EIPS/eip-225#standardized-proof-of-authority) maintains the list of authorized signers fully contained in the block headers. Clique accomplishes this wihtout making any changes to existing block headers.

<b>The only change really needed is to increase the size limit of a block header meta data field to 65 bytes (to fit a secp256k1 signature).</b>

Open questions:
* Are authority public keys not saved on chain? If so do we start with a static number of signers do we update them based on the voting mechanism
* Follow up q: Should the intial set of signers get specified in chain spec config?

### Voting on the list of signers
Specified [here](https://eips.ethereum.org/EIPS/eip-225#specification)

### Authoring a block
Specified [here](https://eips.ethereum.org/EIPS/eip-225#authorizing-a-block)

### How can the btc server read signers

### How can we softfork staking requirments

### Meeting notes

Start with a static list of federation members.
This list of federation members will be updated during block validation.

Scott to research dynafed solution in eth2.0

