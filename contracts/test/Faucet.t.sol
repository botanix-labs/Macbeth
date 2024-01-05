pragma solidity ^0.8.13;

import "forge-std/Test.sol";
import "forge-std/console.sol";
import "../src/Faucet.sol";

contract FaucetTest is Test {
  Faucet public faucet;
  address[] userList;
  address payable user; 

  function setUp() public {
    faucet = new Faucet();
    user = payable(0x7d85b27c2Aa069eE3A4feFbE79F54a3260E3ff9B);
    userList = new address[](1);
    userList[0] = user;
  }

  // requestFunds() tests
  function test_RequestFunds() public {
    deal(address(faucet), 1 ether);
    faucet.requestFundsByList(userList);
  }

  function test_RevertWhen_CallerIsZeroAddress() public {
    deal(address(faucet), 1 ether);
    vm.expectRevert(bytes("Request must not be from zero address"));
    faucet.requestFundsByList(userList);
  }

  function test_RevertWhen_FaucetHasInsufficientFunds() public {
    vm.expectRevert(bytes("Faucet out of funds"));
    faucet.requestFundsByList(userList);
  }

  function test_RevertWhen_UserNeedsToWait() public {
    deal(address(faucet), 1 ether);

    // call 1
    faucet.requestFundsByList(userList);

    // call 2 with insufficient wait time
    vm.expectRevert(bytes("Insufficient time between requests"));
    faucet.requestFundsByList(userList);
  }

  // withdrawFunds() tests
  function test_WithdrawFunds() public {
    deal(address(faucet), 1 ether);

    faucet.setOwner(user);
    vm.prank(user);

    faucet.withdrawFunds();
    assertEq(0, address(faucet).balance);
    assertEq(1 ether, address(user).balance);
  }

  function test_RevertWhen_NonOwnerWithdrawsFunds() public {
    deal(address(faucet), 1 ether);
    vm.expectRevert();
    faucet.withdrawFunds();
  }

  function test_GetNextRequestAt() public {
    deal(address(faucet), 1 ether);
    faucet.requestFundsByList(userList);

    assertEq(61, faucet.getNextRequestAt(user));
  }

  // setOwner() tests
  function test_SetOwner() public {
    deal(address(faucet), 1 ether);
    faucet.setOwner(user);
    assertEq(user, faucet.owner());
  }

  function test_RevertWhen_NonOwnerSetsOwner() public {
    deal(address(faucet), 1 ether);
    vm.prank(user);
    vm.expectRevert("Only the owner can call this function");
    faucet.setOwner(user);
  }

  // setWithdrawAmount() tests
  function test_SetWithdrawAmount() public {
    deal(address(faucet), 1 ether);
    faucet.setWithdrawAmount(123);
    assertEq(123, faucet.withdrawAmount());
  }

  function test_RevertWhen_NonOwnerSetsWithdrawAmount() public {
    deal(address(faucet), 1 ether);
    vm.prank(user);
    vm.expectRevert("Only the owner can call this function");
    faucet.setWithdrawAmount(123);
  }

  // setLockTime() tests
  function test_SetLockTime() public {
    deal(address(faucet), 1 ether);
    faucet.setLockTime(1);
    assertEq(1 * 1 minutes, faucet.lockTime());
  }

  function test_RevertWhen_NonOwnerSetLockTime() public {
    deal(address(faucet), 1 ether);
    vm.prank(user);
    vm.expectRevert("Only the owner can call this function");
    faucet.setLockTime(1);
  }

}