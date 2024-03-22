// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

contract Minting {
    uint public constant SATS_TO_WEI = 10**10;
    uint constant GAS_SIMPLE_TRANSFER = 21000;
    uint constant GAS_AMOUNT_UPDATE = 2003;
    uint constant GAS_REVERT_TRUE = 3;
    uint constant GAS_MINT_AMOUNT_EVENT = 1133;

    /// Some new coins have been minted.
    event Mint(
        address indexed account,
        uint256 amount,
        uint32 bitcoinBlockHeight,
        bytes metadata
    );

    event MintAmount(uint256 indexed amount);

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

        // emit here so gas is included in gasUsed
        emit Mint(destination, amount, bitcoinBlockHeight, metadata);

        // account for gas needed for the transfers, amount update, and require statement if true
        uint256 txCost = (gasStart - gasleft() + GAS_SIMPLE_TRANSFER + GAS_SIMPLE_TRANSFER + GAS_AMOUNT_UPDATE + GAS_REVERT_TRUE + GAS_MINT_AMOUNT_EVENT ) * tx.gasprice;

        // 3 gas for comparison if true
        require(txCost <= amount, "Tx cost exceeds pegin amount");

        // 3 gas for subtraction and 2000 to update the local variable
        amount -= txCost;

        // 21000 gas for each transfer
        payable(destination).transfer(amount);
        payable(refundAddress).transfer(txCost);

        // 375 + 375 * num_topics + 8 * data_size + mem_expansion cost
        // 375 + 375 * 2 + 8 * 1 + 0 = 1133
        emit MintAmount(amount);
    }

    /// Burn coins by sending money to this function.
    /// Burn signature includes parameter for arbitrary bytes data/metadata
    function burn(bytes calldata destination, bytes calldata data) public payable returns (bool success) {
        require(msg.value > 330 * SATS_TO_WEI, "Value must be greater than dust amount of 330 sats/vByte");
        emit Burn(msg.sender, msg.value, destination, data);
        return true;
    }
}
