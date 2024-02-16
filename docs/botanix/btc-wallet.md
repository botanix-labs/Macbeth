
Pegged Bitcoin Wallet
========

The Botanix validators will also be shared custodians of the Bitcoin that
resides withing the Botanix network. The validators will be responsible for
guarding this Bitcoin in a safe manner and paying out pegouts when users
request those.

Botanix is a consensus system, in which all validators come together to agree
on the correct course of action where independent validators have to co-sign
any transaction that tries to spend Bitcoin from the global wallet. This means
that every time a pegout request occurs, the validators need to agree on the
correct course of action. To reduce the attack surface for malicious actors, a
straightforward solution to decide on a course of action is to use a
deterministic algorithm that uses the entire current state of the Bitcoin
wallet, which should be identical for all validators taking part in the
consensus protocol, to calculate the resulting Bitcoin transaction.

Concretely, the state of the Bitcoin wallet consists of, a.o.:

- every UTXO owned by the validators, together with
  - the set of validators that can sign for this UTXO
  - the block the transaction was confirmed in
- the current state of the blockchain so that number of confirmations can be
  calculated

Together with the state, the algorithm will be triggered given a certain input:
- the pegouts that need to be delivered
- the current feerate to use

If all validators can come to consensus on all of the above, we can specify an
algorithm that returns exactly one Bitcoin transaction and exactly the same one
for all validators.

Additionally, it probably makes sense to add a version number to the input so
that the algorithm can be upgraded in the future.


# A Simple Proposal

This document proposes an algorithm that satisfies the above design goals.
It works as follows:

## Parameters

The algorithm will take some hard-coded parameters that all validators have in
common as long as they run software that is aware of the same version of the
protocol. These parameters can be changed by bumping the protocol version
number and upgrading a majority of validators to the new version.

The parameters we will use:

- a maximum number of outputs per transaction
- a maximum number of inputs per transaction
  - (NB this could also be a transaction size limit, but assuming most of our
    UTXOs will have a similar satisfaction weight, a limit on the number might
    be simpler to reason about)


## The Algorithm

Given the input from the above section and the parameters mention, the algorithm
works as follows:

- order all pegout requests by the Botanix block height they were made in
  - ordered from more confirmations to less
  - in case of ex aequo, ordering lexicographically by txid can be used to
    break the tie
- take at most a number of pegouts equal to one less than the maximum number of
  outputs from our parameter
  - leaving space for a change outputs
- take all UTXOs owned by the validators and order them by their Bitcoin
  confirmation height
  - from most confirmations to less
  - ex aequos can be resolved by ordering lexicographically on outpoint
- take UTXOs one by one until either
  - the total input value is sufficient to cover the total output value, plus
    estimated fee, or
  - the maximum number of inputs is reached

The algorithm ends here if enough input value is provided in the transaction, otherwise

- order all remaining UTXOs from highest value to lowest
  - again, ex aequos can be resolved by ordering lexicographically on outpoint
- start replacing the inputs from last to first with the UTXOs from the new
  ordering
  - skip those whose value is higher than the supposed replacement
  - stop as soon as the total input value covers the total output value

The algorithm ends here if enough input value is provided in the transaction, otherwise

- remove the last pegout request from the tx until the total input value
  covers the total output value

This algorithm should always converge to a working tx. Even in the case of zero
pegout outputs, it will result in the construction of ever bigger and bigger
change outputs so that eventually the combined change UTXOs can cover any
pegout request amount.



# Further Improvements

## Deprecating Multisigs

As validators come and go, some multisigs might live longer than others. When a
high enough number of users from a certain multisig leave the system, the
multisig will have to be deprecated and the money tied up inside it will have
to be moved into another multisig.

For this to work, a list of "dying" mulsitigs can be added to the state of the
Bitcoin wallet.


## Deadlocks

Currently the algorithm contains very little varying information and is
actually very reliable when it comes to variation in the outputs. This can be
considered a meritable property as it greatly reduces the gameability of the
algorithm by certain validators that want to push the others into taking a
specific course of action. (F.e. if a strong stochastic element is added, they
could decide to refuse to propose a transaction in the hope that the next epoch
the stochastic element will result in a transaction that is more in their
liking.)

However, the reliability of the results also has a potential drawback that if
for some unexpected reason the algorithm might fail (caution will obviously be
taken in the design of the protocol to maximally try to avoid this
possibility), it might fail repeatedly and the system might get stuck by
forever failing to agree on a payment. Adding a stochastic element can avoid
such a situation because, depending on the rate of influence of the stochastic
element, the resulting transaction can potentially be vastly different from
epoch to epoch.

