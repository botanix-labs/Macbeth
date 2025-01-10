# Minting Contract

The minting contract is a contract that allows users to bridge Bitcoin to the Botanix Layer 2. The contract has two functions:

-   `mint(address destination, uint256 amount, uint32 bitcoinBlockHeight, bytes metadata, address refundAddress)`: This function allows users to mint a specific amount of tokens given there is a valid pegin proof. This proof is validated by the consensus rules of the Botanix Layer 2.
-   `burn(address destination, uint256 amount, bytes destination, bytes metadata)`: This function allows users to burn and/or request some assets back to the Bitcoin Layer 1.

Both functions emit events that are used to track the minting and burning of tokens.

## How to

### Install Dependencies

First install [forge and foundry](https://book.getfoundry.sh/reference/forge/forge-install) for your respective platform.

### Build Contracts

```bash
forge build
```

### Run Tests

```bash
forge test --match-contract Minting -vvv
```

### Run Tests with Coverage

```bash
forge coverage --match-contract Minting -vvv
```
