// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

contract Minting {
    uint public constant SATS_TO_WEI = 10**10;
    uint constant GAS_SIMPLE_TRANSFER = 21000;
    uint constant GAS_AMOUNT_UPDATE = 2003;
    uint constant GAS_REVERT_TRUE = 3;

    // The base gas cost to emit the [Mint] event.
    //
    // Additionally the cost for the [metadata] variable length field
    // shoudl be added.
    //
    // 375 + 2 * 375 (topics) + 8 * 4 (account, amount, mintAmount, height)
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
        bytes metadata,
        uint256 mintAmount
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

        // To estimate the entire tx cost we take the gas used until this point
        // (using gaslest), then add the gas cost for all individual parts below
        // this calculation manually accounted for.
        uint256 metadataGas = metadata.length / 4 + 1;
        uint256 txGas = gasStart - gasleft()
            + GAS_SIMPLE_TRANSFER
            + GAS_SIMPLE_TRANSFER
            + GAS_AMOUNT_UPDATE
            + GAS_REVERT_TRUE
            + BASE_GAS_MINT_EVENT
            + metadataGas;
        uint256 txCost = txGas * tx.gasprice;

        // 3 gas for comparison if true
        require(txCost <= amount, "Tx cost exceeds pegin amount");

        // 3 gas for subtraction and 2000 to update the local variable
        uint256 mintAmount = amount - txCost;

        // 21000 gas for each transfer
        payable(destination).transfer(mintAmount);
        payable(refundAddress).transfer(txCost);

        emit Mint(destination, amount, bitcoinBlockHeight, metadata, mintAmount);
    }

    /// Burn coins by sending money to this function.
    /// Burn signature includes parameter for arbitrary bytes data/metadata
    function burn(bytes calldata destination, bytes calldata data) public payable returns (bool success) {
        require(msg.value > 330 * SATS_TO_WEI, "Value must be greater than dust amount of 330 sats/vByte");
        emit Burn(msg.sender, msg.value, destination, data);
        return true;
    }
}
