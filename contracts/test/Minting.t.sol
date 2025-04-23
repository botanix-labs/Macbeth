pragma solidity ^0.8.13;

import "forge-std/Test.sol";
import "forge-std/console.sol";
import "../src/Minting.sol";

contract MintingTest is Test {
    Minting public minting;

    // mock values
    address payable destination;
    address payable refundAddress;
    uint256 amount;
    uint32 bitcoinBlockHeight;
    uint256 dustThreshold;

    event Mint(
        address indexed account,
        uint256 amount,
        uint32 bitcoinBlockHeight,
        bytes metadata
    );

    function setUp() public {
        minting = new Minting();
        deal(address(minting), 21000000 ether);
        destination = payable(0x31C1ebB34954eEd948949320Ca8a61FAff80C98d);
        refundAddress = payable(0xdEf45aAC371228F5210D12940066f4375C4AF029);
        amount = 1000000000000000000;
        bitcoinBlockHeight = 1;
        dustThreshold = 330 * 10**10; // convert sats to wei
    }


    function testFundContract() public payable {
        // mock metadata
        bytes memory metadata = bytes("0x00000000");
        minting.mint(destination, amount, bitcoinBlockHeight, metadata, refundAddress);
    }

    function testMintEvent() public payable {
        // set gas price to greater than 0
        vm.fee(1);

        // check that the Mint event was emitted
        vm.expectEmit(true, true, true, true);

        // mock metadata
        bytes memory metadata = bytes("0x00000000");
        uint256 expectedMintAmount = 999999999999850000;
        emit Mint(destination, expectedMintAmount, bitcoinBlockHeight, metadata);

        minting.mint(destination, amount, bitcoinBlockHeight, metadata, refundAddress);
    }


    function testRevertWhenTxCostExceedsAmount() public payable {
        // set gas price so txCost exceeds amount
        vm.fee(10000000000000000000000);

        bytes memory metadata = bytes("0x00000000");
        
        vm.expectRevert(); // Expect the transaction to revert
        minting.mint(destination, amount, bitcoinBlockHeight, metadata, refundAddress);
    }

   
    function testBurnDustRequire() public payable {
        bytes memory data = bytes("0x00000000");
        bytes memory destinationBytes = bytes("0x31C1ebB34954eEd948949320Ca8a61FAff80C98d");

        minting.burn{value: dustThreshold + 1}(destinationBytes, data);
    }

    function testRevertWhenBurnDustRequire() public payable {
        bytes memory data = bytes("0x00000000");
        bytes memory destinationBytes = bytes("0x31C1ebB34954eEd948949320Ca8a61FAff80C98d");

        vm.expectRevert(); // Expect the transaction to revert
        minting.burn{value: dustThreshold}(destinationBytes, data);
    }


    // -- Gas Benchmarking Tests --
    /// Used to help determine the `txCost` of the mint() function
    /// which is sent to the refundAddress.

    /// Measure the total gas used by mint() with 160 bytes of metadata
    /// That is 2 bitcoin headers worth of metadata.
    /// run `forge test --gas-report` to see the gas used
    function testFullMintCost() public {
        uint256 length = 160;
        // create a 160‑byte array filled with 0x11
        bytes memory metadata160 = new bytes(length);
        for (uint256 i = 0; i < length; i++) {
            metadata160[i] = 0x11;
        }
        uint256 beforeGas = gasleft();
        minting.mint(destination, amount, bitcoinBlockHeight, metadata160, refundAddress);
        uint256 used = beforeGas - gasleft();
        console.log("full mint() gas:", used);
    }
}
