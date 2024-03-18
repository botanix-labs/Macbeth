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

    event MintAmount(uint256 indexed amount);

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
        // check that the Mint event was emitted
        vm.expectEmit(true, true, true, true);

        // mock metadata
        bytes memory metadata = bytes("0x00000000");
        emit Mint(destination, amount, bitcoinBlockHeight, bytes("0x00000000"));

        minting.mint(destination, amount, bitcoinBlockHeight, metadata, refundAddress);
    }

    function testMintAmountEvent() public payable {
        // set gas price to greater than 0
        vm.txGasPrice(1);

        // check that the Mint event was emitted
        vm.expectEmit(true, false, false, false);

        // mock metadata
        bytes memory metadata = bytes("0x00000000");
        uint256 expectedMintAmount = 999999999999929661;
        emit MintAmount(expectedMintAmount);

        minting.mint(destination, amount, bitcoinBlockHeight, metadata, refundAddress);
    }

    function testFailTxCostExceedsAmount() public payable {
        // set gas price so txCost exceeds amount and call reverts
        vm.txGasPrice(10000000000000000000000);

        bytes memory metadata = bytes("0x00000000");
        minting.mint(destination, amount, bitcoinBlockHeight, metadata, refundAddress);
    }
   
    function testBurnDustRequire() public payable {
        bytes memory data = bytes("0x00000000");
        bytes memory destinationBytes = bytes("0x31C1ebB34954eEd948949320Ca8a61FAff80C98d");

        minting.burn{value: dustThreshold + 1}(destinationBytes, data);
    }

    function testFailBurnDustRequire() public payable {
        bytes memory data = bytes("0x00000000");
        bytes memory destinationBytes = bytes("0x31C1ebB34954eEd948949320Ca8a61FAff80C98d");

        minting.burn{value: dustThreshold}(destinationBytes, data);
    }

}
