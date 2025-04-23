// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

contract MultiMintHelper {
    address public immutable mintingContract;

    constructor(address _mintingContract) {
        mintingContract = _mintingContract;
    }

    /// @notice Calls the minting contract's mint function twice with independent parameters.
    function multiMintTwo(
        address destination1,
        uint256 amount1,
        uint32 bitcoinBlockHeight1,
        bytes memory metadata1,
        address refundAddress1,
        address destination2,
        uint256 amount2,
        uint32 bitcoinBlockHeight2,
        bytes memory metadata2,
        address refundAddress2
    ) public {
        // Call mint for the first pegin using low-level call or assuming ABI compatibility
        // We need to encode the function call data for mint(...)
        bytes memory callData1 = abi.encodeWithSelector(
            bytes4(keccak256("mint(address,uint256,uint32,bytes,address)")),
            destination1,
            amount1,
            bitcoinBlockHeight1,
            metadata1,
            refundAddress1
        );
        (bool success1, ) = mintingContract.call(callData1);
        require(success1, "First mint call failed");

        // Call mint for the second pegin
        bytes memory callData2 = abi.encodeWithSelector(
            bytes4(keccak256("mint(address,uint256,uint32,bytes,address)")),
            destination2,
            amount2,
            bitcoinBlockHeight2,
            metadata2,
            refundAddress2
        );
        (bool success2, ) = mintingContract.call(callData2);
        require(success2, "Second mint call failed");
    }
} 