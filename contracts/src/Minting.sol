// SPDX-License-Identifier: MIT
pragma solidity 0.8.13;

contract Minting {
    uint public constant SATS_TO_WEI = 10**10;
    uint constant GAS_INTERNAL_TRANSFER = 2300;
    uint constant GAS_AMOUNT_UPDATE = 2003;
    uint constant GAS_REVERT_TRUE = 3;

    // The base gas cost to emit the [Mint] event.
    //
    // Additionally the cost for the [metadata] variable length field
    // should be added.
    //
    // 375 + 2 * 375 (topics) + 8 * 4 (account, amount, bitcoinBlockHeight, metadata)
    //
    // metadata is variable length, so only including the first word above 
    //
    // Source: https://www.rareskills.io/post/ethereum-events
    // > 375 + 375 * num_topics + 8 * data_size + mem_expansion cost
    // >
    // > Each event costs at least 375 gas. An additional 375 is paid for each
    // > indexed parameter. A non-anonymous event has the event selector as an
    // > indexed parameter, so that cost is included most of the time. Then we
    // > pay 8 times the number of 32 byte words written to the chain. Because
    // > this region is stored in memory before being emitted, the memory
    // > expansion cost must be accounted for also.
    uint constant BASE_GAS_MINT_EVENT = 1157;

    /// Some new coins have been minted.
    event Mint(
        address indexed account,
        uint256 amount,
        uint32 bitcoinBlockHeight,
        bytes metadata
    );

    /// Some existing coins have been burned.
    event Burn(address indexed account, uint256 amount, bytes destination, bytes metadata);

    /// A mapping from users to their pegin bitcoin block heights.
    mapping(address => uint32) public peginBitcoinBlockHeight;

    /// Mint new coins to the given destination account.
    function mint(
        address destination,
        uint256 amount,
        uint32 bitcoinBlockHeight,
        bytes calldata metadata,
        address refundAddress
    ) public {
        uint256 gasStart = gasleft();

        // Check that the user bitcoin block height is increasing.
        require(
            bitcoinBlockHeight > peginBitcoinBlockHeight[destination],
            "user bitcoinBlockHeight needs to increase"
        );
        peginBitcoinBlockHeight[destination] = bitcoinBlockHeight;

        // account for gas needed for the transfers, amount update, and require statement if true
        // metadata is variable length and the first byte is included in BASE_GAS_MINT_EVENT
        uint256 txCost = 
            (gasStart - gasleft() 
                + GAS_INTERNAL_TRANSFER 
                + GAS_INTERNAL_TRANSFER 
                + GAS_AMOUNT_UPDATE 
                + GAS_REVERT_TRUE 
                + BASE_GAS_MINT_EVENT 
                + metadata.length / 4 - 1) 
            * tx.gasprice;

        // 3 gas for comparison if true
        require(txCost <= amount, "Tx cost exceeds pegin amount");

        // 3 gas for subtraction and 2000 to update the local variable
        amount -= txCost;

        // 2300 gas for each transfer
        payable(destination).transfer(amount);
        payable(refundAddress).transfer(txCost);

        emit Mint(destination, amount, bitcoinBlockHeight, metadata);
    }

    /// Burn coins by sending money to this function.
    /// Burn signature includes parameter for arbitrary bytes data/metadata
    function burn(bytes calldata destination, bytes calldata data) public payable returns (bool success) {
        require(msg.value > 330 * SATS_TO_WEI, "Value must be greater than dust amount of 330 sats/vByte");
        emit Burn(msg.sender, msg.value, destination, data);
        return true;
    }
}
