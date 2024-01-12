// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

contract Minting {
    uint public constant SATS_TO_WEI = 10**10;

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
        bytes calldata metadata
    ) public {
        // Check that the user bitcoin block height is increasing.
        require(
            bitcoinBlockHeight > peginBitcoinBlockHeight[destination],
            "user bitcoinBlockHeight needs to increase"
        );
        peginBitcoinBlockHeight[destination] = bitcoinBlockHeight;

        payable(destination).transfer(amount);
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
