pragma solidity ^0.8.13;

import "forge-std/Test.sol";
import "forge-std/console.sol";
import "../src/Minting.sol";

contract MintingTest is Test {
    Minting public minting;

    // mock values
    address payable destination;
    uint256 amount;
    uint32 bitcoinBlockHeight;
    uint256 dustThreshold;

    function setUp() public {
        minting = new Minting();
        deal(address(minting), 21000000 ether);
        destination = payable(0x31C1ebB34954eEd948949320Ca8a61FAff80C98d);
        amount = 100;
        bitcoinBlockHeight = 1;
        dustThreshold = 330 * 10**10; // convert sats to wei
    }


    function testFundContract() public payable {
        // mock metadata
        bytes memory metadata = bytes("0x00000000");
        minting.mint(destination, amount, bitcoinBlockHeight, metadata);
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
